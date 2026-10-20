//! Versioned per-user application configuration.
//!
//! * Stored as pretty-printed JSON at `<app_config_dir>/config.json` (Windows:
//!   `%APPDATA%\com.lmurace.engineering\config.json`).
//! * `schema_version` is mandatory. A file from a newer schema is never guessed at; it is rejected.
//! * Every load is validated. An unusable file never prevents start-up: safe defaults are used, the problem is
//!   reported to the UI (`ConfigIssue`, returned by `get_config`), and the rejected file is preserved next to
//!   the config as `config.invalid-<UTC timestamp>.json` so nothing the user wrote is lost.
//! * Writes are atomic: write a temporary file in the same directory, fsync, then rename over the target.
//! * Missing fields take their defaults (hand-edited partial files work); unknown fields are rejected (typos
//!   are reported instead of being silently ignored).
//! * `application.environment` is informational: the build environment baked into the binary is
//!   authoritative and always overrides the file (see `app_info::build_environment`).

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::logging::component;

/// Current (and only) config schema. Bump when the format changes, and add a migration in `parse_config`.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const CONFIG_FILE_NAME: &str = "config.json";
/// Refuse to parse anything larger than this; a legitimate config is a few hundred bytes.
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

pub const DEFAULT_RELAY_HOST: &str = "relay.lmuracing.com";
pub const DEFAULT_RELAY_PORT: u16 = 443;
pub const DEFAULT_CONNECT_TIMEOUT_MS: u32 = 5_000;
pub const CONNECT_TIMEOUT_MS_RANGE: (u32, u32) = (500, 60_000);
pub const DEFAULT_FUEL_WINDOW_LAPS: u32 = 5;
/// Fuel windows the fuel engine supports (`race_engines::fuel::FuelWindow`); any other value is rejected.
pub const SUPPORTED_FUEL_WINDOW_LAPS: [u32; 5] = app_core::settings::SUPPORTED_FUEL_WINDOW_LAPS;
pub const DEFAULT_FUEL_RESERVE_LAPS: f64 = 1.0;
pub const FUEL_RESERVE_LAPS_RANGE: (f64, f64) = (0.0, 10.0);
pub const DEFAULT_UI_SCALE: f64 = 1.0;
pub const UI_SCALE_RANGE: (f64, f64) = (0.75, 2.0);
/// UI refresh is capped at 60 Hz (docs/DECISIONS.md D-012).
pub const DEFAULT_UI_MAX_REFRESH_HZ: u32 = 60;
pub const UI_MAX_REFRESH_HZ_RANGE: (u32, u32) = (10, 60);

// ------------------------------------------------------------------------------------------------------------
// Enumerations
// ------------------------------------------------------------------------------------------------------------

/// Operating mode. Both PCs run the same binary; the mode decides the role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Driver,
    Engineer,
}

impl Mode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Driver => "driver",
            Mode::Engineer => "engineer",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Deployment environment. Chosen at build time (`LMURE_ENV`), never at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Development,
    Staging,
    Production,
}

impl Environment {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "development" => Some(Environment::Development),
            "staging" => Some(Environment::Staging),
            "production" => Some(Environment::Production),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Environment::Development => "development",
            Environment::Staging => "staging",
            Environment::Production => "production",
        }
    }

    /// The transport's view of this environment (the relay must announce the same one).
    pub const fn to_transport(self) -> transport::endpoint::Environment {
        match self {
            Environment::Development => transport::endpoint::Environment::Development,
            Environment::Staging => transport::endpoint::Environment::Staging,
            Environment::Production => transport::endpoint::Environment::Production,
        }
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Default for Environment {
    fn default() -> Self {
        crate::app_info::build_environment()
    }
}

// ------------------------------------------------------------------------------------------------------------
// Schema
// ------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub schema_version: u32,
    pub application: ApplicationConfig,
    pub network: NetworkConfig,
    pub telemetry: TelemetryConfig,
    pub fuel: FuelConfig,
    pub ui: UiConfig,
    pub driver: DriverConfig,
    pub engines: EnginesConfig,
    pub window: WindowConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            application: ApplicationConfig::default(),
            network: NetworkConfig::default(),
            telemetry: TelemetryConfig::default(),
            fuel: FuelConfig::default(),
            ui: UiConfig::default(),
            driver: DriverConfig::default(),
            engines: EnginesConfig::default(),
            window: WindowConfig::default(),
        }
    }
}

impl AppConfig {
    /// Safe defaults for the given build environment. `mode` is unset (first-launch selection).
    pub fn defaults_for(environment: Environment) -> Self {
        let mut config = Self::default();
        config.application.environment = environment;
        if environment == Environment::Production {
            config.network.relay_host = DEFAULT_RELAY_HOST.to_string();
        }
        config
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ApplicationConfig {
    /// `None` until the user picks a mode on first launch.
    pub mode: Option<Mode>,
    /// Informational; always overwritten with the build environment.
    pub environment: Environment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    /// Relay host name or IP address. Empty = not configured yet.
    pub relay_host: String,
    pub relay_port: u16,
    pub connect_timeout_ms: u32,
    /// Additional trusted CA certificate (PEM) for self-hosted/dev relays. Only honoured in `development`
    /// builds; see [`NetworkConfig::trusted_extra_ca_file`]. Certificate validation is never disabled.
    pub extra_ca_file: Option<PathBuf>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            relay_host: String::new(),
            relay_port: DEFAULT_RELAY_PORT,
            connect_timeout_ms: DEFAULT_CONNECT_TIMEOUT_MS,
            extra_ca_file: None,
        }
    }
}

impl NetworkConfig {
    /// The extra CA file to trust, or `None` if unset or if `environment` is not `development`.
    pub fn trusted_extra_ca_file(&self, environment: Environment) -> Option<&Path> {
        match environment {
            Environment::Development => self.extra_ca_file.as_deref(),
            Environment::Staging | Environment::Production => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetryConfig {
    pub recording_enabled: bool,
    /// Override for the recording directory. `None` = per-user app data directory.
    pub recording_dir: Option<PathBuf>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            recording_enabled: true,
            recording_dir: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FuelConfig {
    /// Number of recent valid laps averaged for fuel-per-lap: 1, 2, 3, 5 or 10.
    pub window_laps: u32,
    /// Safety margin, in laps of fuel, added to fuel-to-end calculations.
    pub reserve_laps: f64,
}

impl Default for FuelConfig {
    fn default() -> Self {
        Self {
            window_laps: DEFAULT_FUEL_WINDOW_LAPS,
            reserve_laps: DEFAULT_FUEL_RESERVE_LAPS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// Interface zoom factor.
    pub scale: f64,
    /// Upper bound for UI repaint / data push rate.
    pub max_refresh_hz: u32,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            scale: DEFAULT_UI_SCALE,
            max_refresh_hz: DEFAULT_UI_MAX_REFRESH_HZ,
        }
    }
}

/// Driver-mode behaviour.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DriverConfig {
    /// Start streaming automatically once LMU telemetry is available and the user is logged in.
    /// Off by default (owner decision Q7); recording runs regardless (Q9).
    pub auto_stream: bool,
}

/// Race-engine options, applied to the driver pipeline's `EngineHub` at the next driver start (together with
/// `fuel`). Tyre and brake thresholds deliberately have no field here: `race-engines` has no universal default
/// for them (engineer-defined `AlertRule`s).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EnginesConfig {
    /// Evaluate the default alert rules (fuel/energy reserve, stream age, packet loss, latency).
    pub default_alerts_enabled: bool,
}

impl Default for EnginesConfig {
    fn default() -> Self {
        Self {
            default_alerts_enabled: true,
        }
    }
}

/// Main-window geometry, in physical pixels, saved when the window closes and restored at start-up.
/// All fields are optional: `None` keeps the default from `tauri.conf.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    /// Outer position of the top-left corner.
    pub x: Option<i32>,
    pub y: Option<i32>,
    /// Inner (client-area) size.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub maximized: bool,
}

pub const WINDOW_MIN_WIDTH: u32 = 400;
pub const WINDOW_MIN_HEIGHT: u32 = 300;
/// Larger than any real display arrangement; anything beyond is a corrupt value.
pub const WINDOW_MAX_EXTENT: u32 = 32_768;

/// Why the persisted configuration cannot be turned into [`app_core::settings::CoreSettings`].
#[derive(Debug, thiserror::Error)]
pub enum CoreSettingsError {
    #[error("could not read the extra CA file {}: {source}", .path.display())]
    ExtraCaUnreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl NetworkConfig {
    /// The relay address in the form `app_core` expects (`host:port`, `[ipv6]:port`), or `None` if no relay
    /// host is configured.
    pub fn relay_address(&self) -> Option<String> {
        let host = self.relay_host.trim();
        if host.is_empty() {
            return None;
        }
        Some(match host.parse::<IpAddr>() {
            Ok(IpAddr::V6(_)) => format!("[{host}]:{}", self.relay_port),
            _ => format!("{host}:{}", self.relay_port),
        })
    }
}

impl AppConfig {
    /// The headless `app-core` settings for this configuration.
    ///
    /// `default_recording_dir` is used as-is when `telemetry.recording_dir` is unset. The extra CA file is read
    /// only in development builds (see [`NetworkConfig::trusted_extra_ca_file`]); an unreadable file is an
    /// error, never silently ignored.
    pub fn to_core_settings(
        &self,
        default_recording_dir: &Path,
    ) -> Result<app_core::settings::CoreSettings, CoreSettingsError> {
        let mode = match self.application.mode {
            Some(Mode::Engineer) => app_core::bridge::status::AppMode::Engineer,
            Some(Mode::Driver) | None => app_core::bridge::status::AppMode::Driver,
        };

        let extra_ca_pem = match self
            .network
            .trusted_extra_ca_file(self.application.environment)
        {
            Some(path) => Some(std::fs::read(path).map_err(|source| {
                CoreSettingsError::ExtraCaUnreadable {
                    path: path.to_path_buf(),
                    source,
                }
            })?),
            None => None,
        };

        let recording_dir = self
            .telemetry
            .recording_dir
            .clone()
            .unwrap_or_else(|| default_recording_dir.to_path_buf());

        Ok(app_core::settings::CoreSettings {
            mode,
            driver: app_core::settings::DriverSettings {
                auto_stream: self.driver.auto_stream,
                recording_enabled: self.telemetry.recording_enabled,
                recording_dir,
            },
            network: app_core::settings::NetworkSettings {
                relay_url: self.network.relay_address(),
                connect_timeout_ms: self.network.connect_timeout_ms,
                extra_ca_pem,
            },
            ui: app_core::settings::UiSettings {
                max_refresh_hz: self.ui.max_refresh_hz,
                scale: self.ui.scale,
            },
            engines: app_core::settings::EngineSettings {
                default_alerts_enabled: self.engines.default_alerts_enabled,
                fuel_window_laps: self.fuel.window_laps,
                fuel_reserve_laps: self.fuel.reserve_laps,
            },
        })
    }
}

// ------------------------------------------------------------------------------------------------------------
// Validation
// ------------------------------------------------------------------------------------------------------------

/// One validation failure, addressed by dotted field path (e.g. `network.relay_port`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    fn new(field: &str, message: impl Into<String>) -> Self {
        Self {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

fn check_u32_range(errors: &mut Vec<FieldError>, field: &str, value: u32, (min, max): (u32, u32)) {
    if !(min..=max).contains(&value) {
        errors.push(FieldError::new(
            field,
            format!("must be between {min} and {max} (got {value})"),
        ));
    }
}

fn check_f64_range(errors: &mut Vec<FieldError>, field: &str, value: f64, (min, max): (f64, f64)) {
    if !value.is_finite() || !(min..=max).contains(&value) {
        errors.push(FieldError::new(
            field,
            format!("must be a number between {min} and {max} (got {value})"),
        ));
    }
}

fn check_absolute_path(errors: &mut Vec<FieldError>, field: &str, path: Option<&Path>) {
    let Some(path) = path else { return };
    if path.as_os_str().is_empty() {
        errors.push(FieldError::new(
            field,
            "must not be empty (use null for the default)",
        ));
    } else if !path.is_absolute() {
        errors.push(FieldError::new(
            field,
            format!("must be an absolute path (got {})", path.display()),
        ));
    }
}

/// Validate a relay host: empty (not configured), an IP address, or a DNS host name. URLs, paths and ports
/// are rejected with a specific message.
pub fn validate_relay_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Ok(());
    }
    if host.trim() != host {
        return Err("must not have leading or trailing whitespace".to_owned());
    }
    if host.contains("://") || host.contains('/') {
        return Err("must be a host name or IP address, not a URL".to_owned());
    }
    if host.parse::<IpAddr>().is_ok() {
        return Ok(());
    }
    if host.contains(':') {
        return Err("must not include a port (set network.relay_port instead)".to_owned());
    }
    if host.len() > 253 {
        return Err("must be at most 253 characters".to_owned());
    }
    let name = host.strip_suffix('.').unwrap_or(host);
    for label in name.split('.') {
        if label.is_empty() {
            return Err("must not contain empty labels (\"..\")".to_owned());
        }
        if label.len() > 63 {
            return Err("each dot-separated label must be at most 63 characters".to_owned());
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("may only contain ASCII letters, digits, '-' and '.'".to_owned());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("labels must not start or end with '-'".to_owned());
        }
    }
    Ok(())
}

impl AppConfig {
    /// Check every field; returns all problems at once.
    pub fn validate(&self) -> Result<(), Vec<FieldError>> {
        let mut errors = Vec::new();

        if self.schema_version != CURRENT_SCHEMA_VERSION {
            errors.push(FieldError::new(
                "schema_version",
                format!(
                    "must be {CURRENT_SCHEMA_VERSION} (got {})",
                    self.schema_version
                ),
            ));
        }

        if let Err(message) = validate_relay_host(&self.network.relay_host) {
            errors.push(FieldError::new("network.relay_host", message));
        }
        if self.network.relay_port == 0 {
            errors.push(FieldError::new(
                "network.relay_port",
                "must be between 1 and 65535 (got 0)",
            ));
        }
        check_u32_range(
            &mut errors,
            "network.connect_timeout_ms",
            self.network.connect_timeout_ms,
            CONNECT_TIMEOUT_MS_RANGE,
        );
        check_absolute_path(
            &mut errors,
            "network.extra_ca_file",
            self.network.extra_ca_file.as_deref(),
        );

        check_absolute_path(
            &mut errors,
            "telemetry.recording_dir",
            self.telemetry.recording_dir.as_deref(),
        );

        if !SUPPORTED_FUEL_WINDOW_LAPS.contains(&self.fuel.window_laps) {
            errors.push(FieldError::new(
                "fuel.window_laps",
                format!(
                    "must be one of 1, 2, 3, 5 or 10 (got {})",
                    self.fuel.window_laps
                ),
            ));
        }
        check_f64_range(
            &mut errors,
            "fuel.reserve_laps",
            self.fuel.reserve_laps,
            FUEL_RESERVE_LAPS_RANGE,
        );

        check_f64_range(&mut errors, "ui.scale", self.ui.scale, UI_SCALE_RANGE);
        check_u32_range(
            &mut errors,
            "ui.max_refresh_hz",
            self.ui.max_refresh_hz,
            UI_MAX_REFRESH_HZ_RANGE,
        );

        for (field, value) in [
            ("window.width", self.window.width),
            ("window.height", self.window.height),
        ] {
            let min = if field == "window.width" {
                WINDOW_MIN_WIDTH
            } else {
                WINDOW_MIN_HEIGHT
            };
            if let Some(v) = value {
                check_u32_range(&mut errors, field, v, (min, WINDOW_MAX_EXTENT));
            }
        }
        // The extent bound fits in i32, so the cast cannot wrap.
        let max_pos = WINDOW_MAX_EXTENT as i32;
        for (field, value) in [("window.x", self.window.x), ("window.y", self.window.y)] {
            if let Some(v) = value
                && !(-max_pos..=max_pos).contains(&v)
            {
                errors.push(FieldError::new(
                    field,
                    format!("must be between -{max_pos} and {max_pos} (got {v})"),
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

// ------------------------------------------------------------------------------------------------------------
// Parsing
// ------------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigIssueKind {
    /// Not JSON, wrong shape, unknown fields, or failed validation.
    Invalid,
    /// Written by a newer (or otherwise unsupported) schema version.
    UnsupportedVersion,
    /// The file exists but could not be read (I/O error).
    Unreadable,
}

/// Why the stored configuration was not used. Delivered to the UI via `get_config`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigIssue {
    pub kind: ConfigIssueKind,
    pub message: String,
    pub errors: Vec<FieldError>,
    /// Path of the preserved copy of the rejected file (`config.invalid-<timestamp>.json`).
    pub preserved_as: Option<String>,
}

/// A config document that cannot be used, before any file handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub kind: ConfigIssueKind,
    pub message: String,
    pub errors: Vec<FieldError>,
}

impl Rejection {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            kind: ConfigIssueKind::Invalid,
            message: message.into(),
            errors: Vec::new(),
        }
    }
}

/// Parse and validate a config document. Pure: no I/O, no environment override.
pub fn parse_config(text: &str) -> Result<AppConfig, Rejection> {
    // Notepad and some editors write a UTF-8 byte-order mark; it is not JSON.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    let document: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| Rejection::invalid(format!("the file is not valid JSON: {e}")))?;
    let Some(object) = document.as_object() else {
        return Err(Rejection::invalid("the top level must be a JSON object"));
    };
    let version = match object.get("schema_version") {
        None => {
            return Err(Rejection::invalid(
                "the required field `schema_version` is missing",
            ));
        }
        Some(value) => value
            .as_u64()
            .ok_or_else(|| Rejection::invalid("`schema_version` must be a non-negative integer"))?,
    };
    if version > u64::from(CURRENT_SCHEMA_VERSION) {
        return Err(Rejection {
            kind: ConfigIssueKind::UnsupportedVersion,
            message: format!(
                "schema_version {version} was written by a newer version of the application \
                 (this version supports schema_version {CURRENT_SCHEMA_VERSION})"
            ),
            errors: Vec::new(),
        });
    }
    if version == 0 {
        return Err(Rejection::invalid(
            "schema_version 0 is not a valid schema version",
        ));
    }
    // Migrations from older schema versions go here. Version 1 is the first schema, so there are none yet.

    // Parse from the original text (not the Value) so errors carry line/column information.
    let config: AppConfig = serde_json::from_str(text).map_err(|e| {
        Rejection::invalid(format!(
            "the file does not match the configuration schema: {e}"
        ))
    })?;
    config.validate().map_err(|errors| Rejection {
        kind: ConfigIssueKind::Invalid,
        message: format!("{} field(s) failed validation", errors.len()),
        errors,
    })?;
    Ok(config)
}

// ------------------------------------------------------------------------------------------------------------
// Store (file handling)
// ------------------------------------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration is invalid: {}", join_errors(.0))]
    Invalid(Vec<FieldError>),
    #[error("could not serialize configuration: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("could not write configuration file {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn join_errors(errors: &[FieldError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Result of loading: always a usable config, plus the reason if defaults had to be used.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadOutcome {
    pub config: AppConfig,
    pub issue: Option<ConfigIssue>,
}

#[derive(Debug, Clone)]
pub struct ConfigStore {
    dir: PathBuf,
}

impl ConfigStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(CONFIG_FILE_NAME)
    }

    /// Load the config for the given build environment. Never fails: see [`LoadOutcome`].
    pub fn load(&self, environment: Environment) -> LoadOutcome {
        self.load_at(environment, Utc::now())
    }

    /// [`Self::load`] with an explicit clock (used for the preserved-file timestamp).
    pub fn load_at(&self, environment: Environment, now: DateTime<Utc>) -> LoadOutcome {
        let path = self.path();
        let text = match read_config_text(&path) {
            Ok(Some(text)) => text,
            Ok(None) => {
                tracing::info!(
                    component = component::CONFIG,
                    path = %path.display(),
                    "no configuration file; using defaults (first launch)"
                );
                return LoadOutcome {
                    config: AppConfig::defaults_for(environment),
                    issue: None,
                };
            }
            Err(ReadFailure::Rejected(rejection)) => {
                return self.reject(&path, rejection, environment, now);
            }
            Err(ReadFailure::Io(error)) => {
                tracing::error!(
                    component = component::CONFIG,
                    path = %path.display(),
                    error = %error,
                    "configuration file could not be read; using defaults"
                );
                return LoadOutcome {
                    config: AppConfig::defaults_for(environment),
                    issue: Some(ConfigIssue {
                        kind: ConfigIssueKind::Unreadable,
                        message: format!("{} could not be read: {error}", path.display()),
                        errors: Vec::new(),
                        preserved_as: None,
                    }),
                };
            }
        };

        match parse_config(&text) {
            Ok(mut config) => {
                if config.application.environment != environment {
                    tracing::warn!(
                        component = component::CONFIG,
                        file_environment = %config.application.environment,
                        build_environment = %environment,
                        "config file environment differs from the build environment; the build environment wins"
                    );
                    config.application.environment = environment;
                }
                if environment == Environment::Production && config.network.relay_host.trim().is_empty() {
                    config.network.relay_host = DEFAULT_RELAY_HOST.to_string();
                    config.network.relay_port = DEFAULT_RELAY_PORT;
                }
                if config.network.extra_ca_file.is_some()
                    && config.network.trusted_extra_ca_file(environment).is_none()
                {
                    tracing::warn!(
                        component = component::CONFIG,
                        environment = %environment,
                        "network.extra_ca_file is set but ignored outside development builds"
                    );
                }
                tracing::info!(
                    component = component::CONFIG,
                    path = %path.display(),
                    mode = config.application.mode.map(Mode::as_str),
                    "configuration loaded"
                );
                LoadOutcome {
                    config,
                    issue: None,
                }
            }
            Err(rejection) => self.reject(&path, rejection, environment, now),
        }
    }

    fn reject(
        &self,
        path: &Path,
        rejection: Rejection,
        environment: Environment,
        now: DateTime<Utc>,
    ) -> LoadOutcome {
        let mut message = rejection.message;
        let preserved_as = match preserve_invalid_file(path, now) {
            Ok(preserved) => {
                tracing::warn!(
                    component = component::CONFIG,
                    path = %path.display(),
                    preserved_as = %preserved.display(),
                    reason = %message,
                    errors = %join_errors(&rejection.errors),
                    "configuration rejected; using defaults and preserving the rejected file"
                );
                Some(preserved.display().to_string())
            }
            Err(error) => {
                tracing::error!(
                    component = component::CONFIG,
                    path = %path.display(),
                    reason = %message,
                    error = %error,
                    "configuration rejected and the rejected file could not be preserved"
                );
                message = format!("{message} (the rejected file could not be preserved: {error})");
                None
            }
        };
        LoadOutcome {
            config: AppConfig::defaults_for(environment),
            issue: Some(ConfigIssue {
                kind: rejection.kind,
                message,
                errors: rejection.errors,
                preserved_as,
            }),
        }
    }

    /// Validate and atomically persist `config`.
    pub fn save(&self, config: &AppConfig) -> Result<(), ConfigError> {
        config.validate().map_err(ConfigError::Invalid)?;
        let mut json = serde_json::to_string_pretty(config)?;
        json.push('\n');
        fs::create_dir_all(&self.dir).map_err(|source| ConfigError::Io {
            path: self.dir.clone(),
            source,
        })?;
        let path = self.path();
        write_atomic(&path, json.as_bytes()).map_err(|source| ConfigError::Io { path, source })
    }
}

enum ReadFailure {
    Rejected(Rejection),
    Io(io::Error),
}

/// `Ok(None)` if the file does not exist.
fn read_config_text(path: &Path) -> Result<Option<String>, ReadFailure> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ReadFailure::Io(error)),
    };
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ReadFailure::Rejected(Rejection::invalid(format!(
            "the file is {} bytes; the maximum is {MAX_CONFIG_BYTES}",
            metadata.len()
        ))));
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ReadFailure::Io(error)),
    };
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| ReadFailure::Rejected(Rejection::invalid("the file is not valid UTF-8 text")))
}

/// Move a rejected config file aside as `config.invalid-<UTC timestamp>[-n].json` (never overwrites).
fn preserve_invalid_file(path: &Path, now: DateTime<Utc>) -> io::Result<PathBuf> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("config path has no parent directory"))?;
    let stamp = now.format("%Y%m%dT%H%M%SZ");
    for attempt in 0..100u32 {
        let name = if attempt == 0 {
            format!("config.invalid-{stamp}.json")
        } else {
            format!("config.invalid-{stamp}-{attempt}.json")
        };
        let candidate = dir.join(name);
        if candidate.try_exists()? {
            continue;
        }
        fs::rename(path, &candidate)?;
        return Ok(candidate);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "too many preserved config files with the same timestamp",
    ))
}

/// Write `bytes` to `path` atomically: temp file in the same directory → fsync → rename over `path`.
/// A crash at any point leaves either the old file or the new file, never a partial one.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("target path has no parent directory"))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::other("target path has no file name"))?
        .to_string_lossy();
    let temp = dir.join(format!(".{file_name}.tmp-{}", std::process::id()));

    let result = write_synced_then_rename(&temp, path, bytes);
    if result.is_err() {
        // Best effort: never leave stray temp files behind.
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_synced_then_rename(temp: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::File::create(temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    // std::fs::rename replaces an existing target (MoveFileExW / POSIX rename semantics).
    fs::rename(temp, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let store = ConfigStore::new(dir.path().join("config-dir"));
        (dir, store)
    }

    fn write_raw(store: &ConfigStore, text: &str) {
        fs::create_dir_all(store.dir()).expect("create config dir");
        fs::write(store.path(), text).expect("write config");
    }

    fn fixed_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, 12, 34, 56)
            .single()
            .expect("valid time")
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn absolute_path(leaf: &str) -> PathBuf {
        std::env::temp_dir().join(leaf)
    }

    // --- defaults ---------------------------------------------------------------------------------------

    #[test]
    fn defaults_match_specification_and_are_valid() {
        let config = AppConfig::defaults_for(Environment::Production);
        assert_eq!(config.schema_version, 1);
        assert_eq!(config.application.mode, None);
        assert_eq!(config.application.environment, Environment::Production);
        assert_eq!(config.network.relay_host, DEFAULT_RELAY_HOST);
        assert_eq!(config.network.relay_port, 443);
        assert_eq!(config.network.connect_timeout_ms, 5_000);
        assert_eq!(config.network.extra_ca_file, None);
        assert!(config.telemetry.recording_enabled);
        assert_eq!(config.telemetry.recording_dir, None);
        assert_eq!(config.fuel.window_laps, 5);
        assert!((config.fuel.reserve_laps - 1.0).abs() < f64::EPSILON);
        assert!((config.ui.scale - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.ui.max_refresh_hz, 60);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn serialized_form_uses_snake_case_and_lowercase_enums() {
        let mut config = AppConfig::defaults_for(Environment::Staging);
        config.application.mode = Some(Mode::Engineer);
        let value = serde_json::to_value(&config).expect("serialize");
        assert_eq!(value["schema_version"], json!(1));
        assert_eq!(value["application"]["mode"], json!("engineer"));
        assert_eq!(value["application"]["environment"], json!("staging"));
        assert_eq!(value["network"]["relay_port"], json!(443));
        assert_eq!(value["network"]["extra_ca_file"], json!(null));
    }

    // --- round trip -------------------------------------------------------------------------------------

    #[test]
    fn parse_roundtrips_a_fully_populated_config() {
        let config = AppConfig {
            schema_version: 1,
            application: ApplicationConfig {
                mode: Some(Mode::Driver),
                environment: Environment::Development,
            },
            network: NetworkConfig {
                relay_host: "relay.example.com".to_owned(),
                relay_port: 8443,
                connect_timeout_ms: 2_500,
                extra_ca_file: Some(absolute_path("dev-ca.pem")),
            },
            telemetry: TelemetryConfig {
                recording_enabled: false,
                recording_dir: Some(absolute_path("recs")),
            },
            fuel: FuelConfig {
                window_laps: 10,
                reserve_laps: 0.5,
            },
            ui: UiConfig {
                scale: 1.25,
                max_refresh_hz: 30,
            },
            driver: DriverConfig { auto_stream: true },
            engines: EnginesConfig {
                default_alerts_enabled: false,
            },
            window: WindowConfig {
                x: Some(-100),
                y: Some(200),
                width: Some(1920),
                height: Some(1080),
                maximized: true,
            },
        };
        let text = serde_json::to_string_pretty(&config).expect("serialize");
        assert_eq!(parse_config(&text), Ok(config));
    }

    // --- Lane H sections (driver / engines / window) ---------------------------------------------------------

    #[test]
    fn schema_1_file_without_new_sections_loads_with_defaults() {
        // A config written before the driver/engines/window sections existed: still schema 1 (no migration).
        let config = parse_config(
            r#"{ "schema_version": 1, "application": { "mode": "driver" }, "ui": { "scale": 1.5 } }"#,
        )
        .expect("old schema-1 file must load");
        assert_eq!(config.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(config.ui.scale, 1.5);
        assert_eq!(config.driver, DriverConfig::default());
        assert!(!config.driver.auto_stream, "auto-stream defaults OFF (Q7)");
        assert!(
            config.telemetry.recording_enabled,
            "recording defaults ON (Q9)"
        );
        assert_eq!(config.engines, EnginesConfig::default());
        assert_eq!(config.window, WindowConfig::default());
    }

    #[test]
    fn partial_new_sections_fill_missing_fields_and_reject_unknown_ones() {
        let config =
            parse_config(r#"{ "schema_version": 1, "driver": {}, "window": { "width": 1280 } }"#)
                .expect("partial sections");
        assert_eq!(config.window.width, Some(1280));
        assert_eq!(config.window.height, None);

        let rejected =
            parse_config(r#"{ "schema_version": 1, "engines": { "tyre_wear_alert_pct": 15 } }"#)
                .expect_err("invented fields are rejected");
        assert_eq!(rejected.kind, ConfigIssueKind::Invalid);
    }

    #[test]
    fn fuel_window_must_be_a_supported_size() {
        for laps in SUPPORTED_FUEL_WINDOW_LAPS {
            let mut config = AppConfig::defaults_for(Environment::Development);
            config.fuel.window_laps = laps;
            assert_eq!(config.validate(), Ok(()), "{laps} laps is supported");
        }
        for laps in [0, 4, 6, 8, 20, 50] {
            let mut config = AppConfig::defaults_for(Environment::Development);
            config.fuel.window_laps = laps;
            let errors = config.validate().expect_err("unsupported window");
            assert_eq!(errors[0].field, "fuel.window_laps");
        }
    }

    #[test]
    fn window_geometry_is_validated() {
        let mut config = AppConfig::defaults_for(Environment::Development);
        config.window.width = Some(WINDOW_MIN_WIDTH - 1);
        config.window.height = Some(WINDOW_MAX_EXTENT + 1);
        config.window.x = Some(-(WINDOW_MAX_EXTENT as i32) - 1);
        let errors = config.validate().expect_err("invalid geometry");
        let fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, vec!["window.width", "window.height", "window.x"]);
    }

    #[test]
    fn core_settings_map_every_field() {
        let mut config = AppConfig::defaults_for(Environment::Production);
        config.application.mode = Some(Mode::Engineer);
        config.network.relay_host = "relay.example.com".to_owned();
        config.network.relay_port = 8443;
        config.network.connect_timeout_ms = 2_000;
        config.driver.auto_stream = true;
        config.telemetry.recording_enabled = false;
        config.ui.max_refresh_hz = 30;
        config.ui.scale = 1.25;
        config.fuel.window_laps = 3;
        config.fuel.reserve_laps = 2.5;
        config.engines.default_alerts_enabled = false;
        let default_dir = absolute_path("data").join("recordings");

        let core = config
            .to_core_settings(&default_dir)
            .expect("core settings");
        assert_eq!(core.mode, app_core::bridge::status::AppMode::Engineer);
        assert_eq!(
            core.network.relay_url.as_deref(),
            Some("relay.example.com:8443")
        );
        assert_eq!(core.network.connect_timeout_ms, 2_000);
        assert_eq!(core.network.extra_ca_pem, None);
        assert!(core.driver.auto_stream);
        assert!(!core.driver.recording_enabled);
        assert_eq!(
            core.driver.recording_dir, default_dir,
            "default dir used as-is"
        );
        assert_eq!(core.ui.max_refresh_hz, 30);
        assert_eq!(core.ui.scale, 1.25);
        assert_eq!(core.engines.fuel_window_laps, 3);
        assert!((core.engines.fuel_reserve_laps - 2.5).abs() < f64::EPSILON);
        assert!(!core.engines.default_alerts_enabled);
        assert_eq!(core.validate(), Ok(()));

        config.telemetry.recording_dir = Some(absolute_path("custom"));
        let core = config
            .to_core_settings(&default_dir)
            .expect("core settings");
        assert_eq!(core.driver.recording_dir, absolute_path("custom"));

        config.application.mode = None;
        let core = config
            .to_core_settings(&default_dir)
            .expect("core settings");
        assert_eq!(core.mode, app_core::bridge::status::AppMode::Driver);
    }

    #[test]
    fn relay_address_handles_unset_ipv4_ipv6_and_names() {
        let mut network = NetworkConfig::default();
        assert_eq!(network.relay_address(), None);
        network.relay_host = "10.0.0.5".to_owned();
        assert_eq!(network.relay_address().as_deref(), Some("10.0.0.5:443"));
        network.relay_host = "::1".to_owned();
        network.relay_port = 9000;
        assert_eq!(network.relay_address().as_deref(), Some("[::1]:9000"));
        network.relay_host = "relay.example.com".to_owned();
        assert_eq!(
            network.relay_address().as_deref(),
            Some("relay.example.com:9000")
        );

        // The address must be accepted by the transport's parser.
        for host in ["10.0.0.5", "::1", "relay.example.com"] {
            network.relay_host = host.to_owned();
            let address = network.relay_address().expect("address");
            transport::endpoint::RelayEndpoint::parse(
                &address,
                "test",
                transport::endpoint::Environment::Development,
            )
            .unwrap_or_else(|e| panic!("{address:?} rejected: {e}"));
        }
    }

    #[test]
    fn production_defaults_and_load_default_relay_host() {
        let prod_defaults = AppConfig::defaults_for(Environment::Production);
        assert_eq!(prod_defaults.network.relay_host, DEFAULT_RELAY_HOST);
        assert_eq!(prod_defaults.network.relay_port, DEFAULT_RELAY_PORT);
        assert_eq!(
            prod_defaults.network.relay_address().as_deref(),
            Some("relay.lmuracing.com:443")
        );

        let (tmp, store) = store();
        let empty_host_json = r#"{
            "schema_version": 1,
            "application": { "mode": "driver", "environment": "production" },
            "network": { "relay_host": "", "relay_port": 443 }
        }"#;
        write_raw(&store, empty_host_json);
        let loaded = store.load(Environment::Production);
        assert_eq!(loaded.config.network.relay_host, DEFAULT_RELAY_HOST);
        assert_eq!(
            loaded.config.network.relay_address().as_deref(),
            Some("relay.lmuracing.com:443")
        );
        drop(tmp);
    }

    #[test]
    fn unreadable_extra_ca_is_an_error_in_development_and_ignored_elsewhere() {
        let (tmp, store) = store();
        let missing = tmp.path().join("missing-ca.pem");
        let mut config = AppConfig::defaults_for(Environment::Development);
        config.network.extra_ca_file = Some(missing.clone());
        let error = config
            .to_core_settings(store.dir())
            .expect_err("missing CA file must not be ignored");
        assert!(
            matches!(error, CoreSettingsError::ExtraCaUnreadable { ref path, .. } if *path == missing)
        );

        std::fs::write(&missing, b"-----BEGIN CERTIFICATE-----\n").expect("write");
        let core = config.to_core_settings(store.dir()).expect("readable");
        assert_eq!(
            core.network.extra_ca_pem.as_deref(),
            Some(&b"-----BEGIN CERTIFICATE-----\n"[..])
        );

        // Outside development the file is never read (D-016), even if it is missing.
        std::fs::remove_file(&missing).expect("remove");
        config.application.environment = Environment::Production;
        let core = config
            .to_core_settings(store.dir())
            .expect("production ignores CA");
        assert_eq!(core.network.extra_ca_pem, None);
    }

    #[test]
    fn save_then_load_roundtrips_and_leaves_no_temp_files() {
        let (_tmp, store) = store();
        let mut config = AppConfig::defaults_for(Environment::Development);
        config.application.mode = Some(Mode::Engineer);
        config.network.relay_host = "10.0.0.5".to_owned();

        store.save(&config).expect("save");
        let outcome = store.load(Environment::Development);

        assert_eq!(outcome.issue, None);
        assert_eq!(outcome.config, config);
        assert_eq!(files_in(store.dir()), vec![CONFIG_FILE_NAME.to_owned()]);
    }

    #[test]
    fn save_replaces_an_existing_file_atomically() {
        let (_tmp, store) = store();
        let mut config = AppConfig::defaults_for(Environment::Development);
        config.application.mode = Some(Mode::Driver);
        store.save(&config).expect("first save");
        config.application.mode = Some(Mode::Engineer);
        store.save(&config).expect("second save");

        let on_disk = fs::read_to_string(store.path()).expect("read");
        assert!(on_disk.contains("\"engineer\""));
        assert_eq!(files_in(store.dir()), vec![CONFIG_FILE_NAME.to_owned()]);
    }

    #[test]
    fn save_refuses_an_invalid_config_and_keeps_the_existing_file() {
        let (_tmp, store) = store();
        let good = AppConfig::defaults_for(Environment::Development);
        store.save(&good).expect("save");
        let before = fs::read_to_string(store.path()).expect("read");

        let mut bad = good.clone();
        bad.network.relay_port = 0;
        let error = store.save(&bad).expect_err("must refuse");
        assert!(
            matches!(error, ConfigError::Invalid(ref errors) if errors[0].field == "network.relay_port")
        );
        assert_eq!(fs::read_to_string(store.path()).expect("read"), before);
    }

    // --- loading ----------------------------------------------------------------------------------------

    #[test]
    fn missing_file_yields_defaults_without_issue_and_writes_nothing() {
        let (_tmp, store) = store();
        let outcome = store.load(Environment::Production);
        assert_eq!(
            outcome,
            LoadOutcome {
                config: AppConfig::defaults_for(Environment::Production),
                issue: None
            }
        );
        assert!(!store.dir().exists());
    }

    #[test]
    fn partial_file_is_completed_with_defaults() {
        let (_tmp, store) = store();
        write_raw(
            &store,
            r#"{ "schema_version": 1, "application": { "mode": "driver" } }"#,
        );
        let outcome = store.load(Environment::Development);
        assert_eq!(outcome.issue, None);
        let mut expected = AppConfig::defaults_for(Environment::Development);
        expected.application.mode = Some(Mode::Driver);
        assert_eq!(outcome.config, expected);
    }

    #[test]
    fn utf8_bom_is_accepted() {
        let (_tmp, store) = store();
        write_raw(&store, "\u{feff}{ \"schema_version\": 1 }");
        assert_eq!(store.load(Environment::Development).issue, None);
    }

    #[test]
    fn corrupt_json_yields_defaults_issue_and_preserved_copy() {
        let (_tmp, store) = store();
        let garbage = "{ \"schema_version\": 1, \"application\": ";
        write_raw(&store, garbage);

        let outcome = store.load_at(Environment::Development, fixed_time());

        assert_eq!(
            outcome.config,
            AppConfig::defaults_for(Environment::Development)
        );
        let issue = outcome.issue.expect("issue reported");
        assert_eq!(issue.kind, ConfigIssueKind::Invalid);
        assert!(
            issue.message.contains("not valid JSON"),
            "{}",
            issue.message
        );
        let preserved = PathBuf::from(issue.preserved_as.expect("preserved"));
        assert_eq!(
            preserved.file_name().and_then(|n| n.to_str()),
            Some("config.invalid-20260930T123456Z.json")
        );
        assert_eq!(
            fs::read_to_string(&preserved).expect("read preserved"),
            garbage
        );
        assert!(
            !store.path().exists(),
            "the rejected file must be moved aside"
        );
    }

    #[test]
    fn preserved_copies_never_overwrite_each_other() {
        let (_tmp, store) = store();
        write_raw(&store, "not json 1");
        let first = store.load_at(Environment::Development, fixed_time());
        write_raw(&store, "not json 2");
        let second = store.load_at(Environment::Development, fixed_time());

        let first = first
            .issue
            .and_then(|i| i.preserved_as)
            .expect("first preserved");
        let second = second
            .issue
            .and_then(|i| i.preserved_as)
            .expect("second preserved");
        assert_ne!(first, second);
        assert!(
            second.ends_with("config.invalid-20260930T123456Z-1.json"),
            "{second}"
        );
        assert_eq!(fs::read_to_string(first).expect("read"), "not json 1");
        assert_eq!(fs::read_to_string(second).expect("read"), "not json 2");
    }

    #[test]
    fn validation_failures_are_all_reported_with_field_paths() {
        let (_tmp, store) = store();
        let document = json!({
            "schema_version": 1,
            "application": { "mode": "engineer" },
            "network": {
                "relay_host": "https://relay.example.com",
                "relay_port": 0,
                "connect_timeout_ms": 10,
                "extra_ca_file": "relative\\ca.pem"
            },
            "telemetry": { "recording_dir": "" },
            "fuel": { "window_laps": 0, "reserve_laps": -1.0 },
            "ui": { "scale": 5.0, "max_refresh_hz": 120 }
        });
        write_raw(&store, &document.to_string());

        let outcome = store.load_at(Environment::Development, fixed_time());
        assert_eq!(
            outcome.config,
            AppConfig::defaults_for(Environment::Development)
        );
        let issue = outcome.issue.expect("issue reported");
        assert_eq!(issue.kind, ConfigIssueKind::Invalid);
        let fields: Vec<&str> = issue.errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(
            fields,
            vec![
                "network.relay_host",
                "network.relay_port",
                "network.connect_timeout_ms",
                "network.extra_ca_file",
                "telemetry.recording_dir",
                "fuel.window_laps",
                "fuel.reserve_laps",
                "ui.scale",
                "ui.max_refresh_hz",
            ]
        );
        assert!(issue.preserved_as.is_some());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let rejection =
            parse_config(r#"{ "schema_version": 1, "network": { "relay_prot": 443 } }"#)
                .expect_err("typo must be rejected");
        assert_eq!(rejection.kind, ConfigIssueKind::Invalid);
        assert!(
            rejection.message.contains("relay_prot"),
            "{}",
            rejection.message
        );
    }

    #[test]
    fn unknown_mode_is_rejected() {
        let rejection =
            parse_config(r#"{ "schema_version": 1, "application": { "mode": "pilot" } }"#)
                .expect_err("reject");
        assert!(rejection.message.contains("pilot"), "{}", rejection.message);
    }

    #[test]
    fn schema_version_is_required_and_checked() {
        let missing =
            parse_config(r#"{ "application": { "mode": "driver" } }"#).expect_err("missing");
        assert_eq!(missing.kind, ConfigIssueKind::Invalid);
        assert!(missing.message.contains("schema_version"));

        let zero = parse_config(r#"{ "schema_version": 0 }"#).expect_err("zero");
        assert_eq!(zero.kind, ConfigIssueKind::Invalid);

        let text = parse_config(r#"{ "schema_version": "1" }"#).expect_err("string");
        assert_eq!(text.kind, ConfigIssueKind::Invalid);

        let not_object = parse_config("[1]").expect_err("array");
        assert_eq!(not_object.kind, ConfigIssueKind::Invalid);
    }

    #[test]
    fn newer_schema_version_is_unsupported_and_preserved() {
        let (_tmp, store) = store();
        write_raw(&store, r#"{ "schema_version": 2, "future_section": {} }"#);
        let outcome = store.load_at(Environment::Development, fixed_time());
        let issue = outcome.issue.expect("issue");
        assert_eq!(issue.kind, ConfigIssueKind::UnsupportedVersion);
        assert!(issue.preserved_as.is_some());
        assert_eq!(
            outcome.config,
            AppConfig::defaults_for(Environment::Development)
        );
    }

    #[test]
    fn oversized_file_is_rejected_without_parsing() {
        let (_tmp, store) = store();
        let padding = " ".repeat(usize::try_from(MAX_CONFIG_BYTES).expect("fits") + 1);
        write_raw(&store, &format!("{{\"schema_version\":1}}{padding}"));
        let issue = store
            .load_at(Environment::Development, fixed_time())
            .issue
            .expect("issue");
        assert_eq!(issue.kind, ConfigIssueKind::Invalid);
        assert!(issue.message.contains("maximum"), "{}", issue.message);
    }

    #[test]
    fn non_utf8_file_is_rejected() {
        let (_tmp, store) = store();
        fs::create_dir_all(store.dir()).expect("mkdir");
        fs::write(store.path(), [0xff, 0xfe, 0x00, 0x7b]).expect("write");
        let issue = store
            .load_at(Environment::Development, fixed_time())
            .issue
            .expect("issue");
        assert!(issue.message.contains("UTF-8"), "{}", issue.message);
        assert!(issue.preserved_as.is_some());
    }

    // --- environment ------------------------------------------------------------------------------------

    #[test]
    fn build_environment_overrides_the_file() {
        let (_tmp, store) = store();
        write_raw(
            &store,
            r#"{ "schema_version": 1, "application": { "environment": "development" } }"#,
        );
        let outcome = store.load(Environment::Production);
        assert_eq!(outcome.issue, None);
        assert_eq!(
            outcome.config.application.environment,
            Environment::Production
        );
    }

    #[test]
    fn extra_ca_file_is_only_honoured_in_development() {
        let network = NetworkConfig {
            extra_ca_file: Some(absolute_path("dev-ca.pem")),
            ..NetworkConfig::default()
        };
        assert_eq!(
            network.trusted_extra_ca_file(Environment::Development),
            network.extra_ca_file.as_deref()
        );
        assert_eq!(network.trusted_extra_ca_file(Environment::Staging), None);
        assert_eq!(network.trusted_extra_ca_file(Environment::Production), None);
        assert_eq!(
            NetworkConfig::default().trusted_extra_ca_file(Environment::Development),
            None
        );
    }

    #[test]
    fn environment_parse_roundtrip() {
        for env in [
            Environment::Development,
            Environment::Staging,
            Environment::Production,
        ] {
            assert_eq!(Environment::parse(env.as_str()), Some(env));
        }
        assert_eq!(Environment::parse("prod"), None);
    }

    // --- relay host -------------------------------------------------------------------------------------

    #[test]
    fn relay_host_validation() {
        let long_label = "a".repeat(64);
        let long_name = format!("{}.com", ["abcdefghij"; 25].join("."));
        for ok in [
            "",
            "relay.example.com",
            "relay.example.com.",
            "localhost",
            "a-b.c",
            "10.0.0.1",
            "::1",
            "fe80::1",
        ] {
            assert_eq!(validate_relay_host(ok), Ok(()), "{ok:?} should be valid");
        }
        for bad in [
            "https://relay.example.com",
            "relay.example.com/path",
            "relay.example.com:443",
            " relay.example.com",
            "relay..example.com",
            "-relay.example.com",
            "relay-.example.com",
            "relay_host.example.com",
            "has space.com",
            long_label.as_str(),
            long_name.as_str(),
        ] {
            assert!(
                validate_relay_host(bad).is_err(),
                "{bad:?} should be invalid"
            );
        }
    }
}

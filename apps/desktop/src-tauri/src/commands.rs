//! Tauri commands: the complete, explicit IPC surface of the WebView.
//!
//! Rules: one typed command per operation, no generic "execute"/"invoke anything" entry point, no shell or
//! filesystem access for the frontend. Every command must also be listed in `build.rs` (`COMMANDS`), granted
//! in `capabilities/default.json`, registered in `lib.rs` (`generate_handler!`) and mirrored in
//! `src/lib/ipc.ts`; `tests::command_lists_agree` enforces this.
//!
//! Every failure is a [`CommandError`] with a stable `code` (see [`codes`]); no command reports success for an
//! operation it did not perform. Secrets (passwords) arrive as [`Secret`] and are never logged; connection codes
//! are never logged or persisted.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::{Channel, InvokeResponseBody};

use app_core::bridge::sink::{SubscriptionId, UiSink};
use app_core::bridge::status::{AppMode, CoreStatus};
use app_core::core::{AppCore, CoreError};
use protocol::messages::{ClientRole, Secret};
use synthetic::Scenario;
use transport::auth::{AccountInfo, LoginError};
use transport::connectivity::test_relay_connection as run_connectivity_test;
use transport::engineer::{IdentityCard, JoinError};

use crate::app_info::AppInfo;
use crate::config::{AppConfig, ConfigError, Mode};
use crate::core_bridge::{BridgeSubscribeResponse, ChannelSink};
use crate::logging::component;
use crate::state::{AppState, ConfigSnapshot, RelayReadiness};

/// Every command name, in registration order. Must equal `build.rs` `COMMANDS`, the capability allow-list, the
/// `generate_handler!` list in `lib.rs` and `CommandName` in `src/lib/ipc.ts` (checked by a test).
pub const COMMAND_NAMES: [&str; 28] = [
    "get_app_info",
    "get_config",
    "set_mode",
    "update_settings",
    "get_core_status",
    "test_relay_connection",
    "log_ui_error",
    "login",
    "register_account",
    "logout",
    "get_login_state",
    "driver_start_streaming",
    "driver_stop_streaming",
    "driver_end_session",
    "driver_new_code",
    "driver_kick_engineer",
    "driver_set_recording",
    "driver_add_marker",
    "driver_ack_alert",
    "engineer_join",
    "engineer_confirm",
    "engineer_cancel",
    "engineer_disconnect",
    "engineer_request_snapshot",
    "demo_start",
    "demo_stop",
    "bridge_subscribe",
    "bridge_unsubscribe",
];

/// Stable error codes (mirrored as `CommandErrorCode` in `src/lib/ipc.ts`).
pub mod codes {
    pub const CONFIG_INVALID: &str = "config_invalid";
    pub const CONFIG_SERIALIZE_FAILED: &str = "config_serialize_failed";
    pub const CONFIG_WRITE_FAILED: &str = "config_write_failed";
    pub const INVALID_INPUT: &str = "invalid_input";
    pub const NOT_IMPLEMENTED: &str = "not_implemented";
    pub const RELAY_NOT_CONFIGURED: &str = "relay_not_configured";
    pub const RESTART_REQUIRED: &str = "restart_required";
    pub const CREDENTIAL_STORE_FAILED: &str = "credential_store_failed";
    pub const BRIDGE_CLOSED: &str = "bridge_closed";
    pub const DEMO_ACTIVE: &str = "demo_active";
    pub const UNKNOWN_SCENARIO: &str = "unknown_scenario";
    // Core.
    pub const CORE_SHUTDOWN: &str = "core_shutdown";
    pub const INVALID_MODE: &str = "invalid_mode";
    pub const DRIVER_NOT_RUNNING: &str = "driver_not_running";
    pub const DRIVER_ALREADY_RUNNING: &str = "driver_already_running";
    pub const ENGINEER_NOT_RUNNING: &str = "engineer_not_running";
    pub const ENGINEER_ALREADY_RUNNING: &str = "engineer_already_running";
    pub const DEMO_ERROR: &str = "demo_error";
    pub const SETTINGS_ERROR: &str = "settings_error";
    pub const PIPELINE_ERROR: &str = "pipeline_error";
    pub const RUNTIME_ERROR: &str = "runtime_error";
    pub const IO_ERROR: &str = "io_error";
    pub const RECORDING_UNAVAILABLE: &str = "recording_unavailable";
    pub const NETWORK_UNAVAILABLE: &str = "network_unavailable";
    // Join (engineer).
    pub const CODE_MALFORMED: &str = "code_malformed";
    pub const CODE_INVALID: &str = "code_invalid";
    pub const CODE_EXPIRED: &str = "code_expired";
    pub const SESSION_FULL: &str = "session_full";
    pub const SESSION_CLOSED: &str = "session_closed";
    pub const NOT_AN_ENGINEER: &str = "not_an_engineer";
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const RELOGIN_REQUIRED: &str = "relogin_required";
    pub const RELAY_UNREACHABLE: &str = "relay_unreachable";
    pub const RELAY_INCOMPATIBLE: &str = "relay_incompatible";
    pub const ENVIRONMENT_MISMATCH: &str = "environment_mismatch";
    pub const JOIN_REFUSED: &str = "join_refused";
    pub const JOIN_BUSY: &str = "join_busy";
    pub const JOIN_CANCELLED: &str = "join_cancelled";
    pub const CLIENT_CLOSED: &str = "client_closed";
    // Login.
    pub const CONNECT_FAILED: &str = "connect_failed";
    pub const INVALID_CREDENTIALS: &str = "invalid_credentials";
    pub const AUTH_REFUSED: &str = "auth_refused";
    pub const ROLE_MISMATCH: &str = "role_mismatch";
    pub const HANDSHAKE_FAILED: &str = "handshake_failed";
}

/// Error payload returned to the frontend: a stable machine-readable `code` and a human-readable `message`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl CommandError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<ConfigError> for CommandError {
    fn from(error: ConfigError) -> Self {
        let code = match error {
            ConfigError::Invalid(_) => codes::CONFIG_INVALID,
            ConfigError::Serialize(_) => codes::CONFIG_SERIALIZE_FAILED,
            ConfigError::Io { .. } => codes::CONFIG_WRITE_FAILED,
        };
        Self::new(code, error.to_string())
    }
}

impl From<JoinError> for CommandError {
    fn from(error: JoinError) -> Self {
        let code = match &error {
            JoinError::CodeMalformed(_) => codes::CODE_MALFORMED,
            JoinError::CodeInvalid => codes::CODE_INVALID,
            JoinError::CodeExpired => codes::CODE_EXPIRED,
            JoinError::SessionFull { .. } => codes::SESSION_FULL,
            JoinError::SessionClosed => codes::SESSION_CLOSED,
            JoinError::NotAnEngineer => codes::NOT_AN_ENGINEER,
            JoinError::RateLimited => codes::RATE_LIMITED,
            JoinError::ReloginRequired(_) => codes::RELOGIN_REQUIRED,
            JoinError::Unreachable { .. } => codes::RELAY_UNREACHABLE,
            JoinError::Incompatible { .. } => codes::RELAY_INCOMPATIBLE,
            JoinError::EnvironmentMismatch { .. } => codes::ENVIRONMENT_MISMATCH,
            JoinError::Refused { .. } => codes::JOIN_REFUSED,
            JoinError::Busy => codes::JOIN_BUSY,
            JoinError::Cancelled => codes::JOIN_CANCELLED,
            JoinError::ClientClosed => codes::CLIENT_CLOSED,
        };
        // `message()` is the user-facing text; none of the variants carries the code itself.
        Self::new(code, error.message())
    }
}

impl From<CoreError> for CommandError {
    fn from(error: CoreError) -> Self {
        let code = match error {
            CoreError::Join(join) => return Self::from(join),
            CoreError::ShutDown => codes::CORE_SHUTDOWN,
            CoreError::InvalidMode(_) => codes::INVALID_MODE,
            CoreError::DriverNotRunning => codes::DRIVER_NOT_RUNNING,
            CoreError::DriverAlreadyRunning => codes::DRIVER_ALREADY_RUNNING,
            CoreError::EngineerNotRunning => codes::ENGINEER_NOT_RUNNING,
            CoreError::EngineerAlreadyRunning => codes::ENGINEER_ALREADY_RUNNING,
            CoreError::Demo(_) => codes::DEMO_ERROR,
            CoreError::Settings(_) => codes::SETTINGS_ERROR,
            CoreError::Pipeline(_) => codes::PIPELINE_ERROR,
            CoreError::Runtime(_) => codes::RUNTIME_ERROR,
            CoreError::Io(_) => codes::IO_ERROR,
            CoreError::RecorderUnavailable => codes::RECORDING_UNAVAILABLE,
            CoreError::NetworkUnavailable => codes::NETWORK_UNAVAILABLE,
        };
        Self::new(code, error.to_string())
    }
}

impl From<LoginError> for CommandError {
    fn from(error: LoginError) -> Self {
        let code = match &error {
            LoginError::Connect(_) => codes::CONNECT_FAILED,
            LoginError::InvalidCredentials => codes::INVALID_CREDENTIALS,
            LoginError::Refused { .. } => codes::AUTH_REFUSED,
            LoginError::RoleMismatch { .. } => codes::ROLE_MISMATCH,
            LoginError::Handshake(_) => codes::HANDSHAKE_FAILED,
        };
        Self::new(code, error.to_string())
    }
}

/// Log a failed command (without arguments: they may contain codes or account names) and pass the error on.
fn logged(command: &'static str, error: CommandError) -> CommandError {
    tracing::warn!(
        component = component::IPC,
        command,
        code = error.code,
        "command failed"
    );
    error
}

// ------------------------------------------------------------------------------------------------------------
// DTOs
// ------------------------------------------------------------------------------------------------------------

/// A frontend error report (`log_ui_error`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UiErrorEntry {
    pub message: String,
    pub stack: Option<String>,
    pub component: Option<String>,
}

/// Result of `log_ui_error`: whether this entry was written, and how many earlier ones the rate limit dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UiErrorLogged {
    pub logged: bool,
    /// Entries dropped by the rate limit since the last written entry (reported in that entry's log line).
    pub suppressed: u64,
}

/// Account role as the UI sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleDto {
    Driver,
    Engineer,
}

impl From<ClientRole> for RoleDto {
    fn from(role: ClientRole) -> Self {
        match role {
            ClientRole::Driver => RoleDto::Driver,
            ClientRole::Engineer => RoleDto::Engineer,
        }
    }
}

impl From<Mode> for ClientRole {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Driver => ClientRole::Driver,
            Mode::Engineer => ClientRole::Engineer,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LoginStateDto {
    /// A refresh token for the active mode's role exists (in memory or in the credential store).
    pub authenticated: bool,
    /// The role checked (the active mode; driver before a mode was picked).
    pub role: RoleDto,
    /// Account details, known after a login in this run (not restored from the credential store).
    pub account: Option<AccountInfoDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountInfoDto {
    /// 32 lowercase hex digits.
    pub account: String,
    pub username: String,
    pub display_name: String,
    pub role: RoleDto,
}

impl From<AccountInfo> for AccountInfoDto {
    fn from(info: AccountInfo) -> Self {
        let account = info.account.0.iter().map(|b| format!("{b:02x}")).collect();
        Self {
            account,
            username: info.username,
            display_name: info.display_name,
            role: info.role.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IdentityCardDto {
    pub session_id: u64,
    pub driver_display_name: String,
    pub driver_username: String,
    pub car: Option<String>,
    pub vehicle_class: Option<String>,
    pub track: Option<String>,
    /// LMU session type name (e.g. `Race`), if known.
    pub session_type: Option<String>,
    pub started_at_ns: u64,
    pub schema_version: u16,
}

impl From<IdentityCard> for IdentityCardDto {
    fn from(card: IdentityCard) -> Self {
        Self {
            session_id: card.session_id,
            driver_display_name: card.driver_display_name,
            driver_username: card.driver_username,
            car: card.car,
            vehicle_class: card.vehicle_class,
            track: card.track,
            session_type: card.session_type.map(|st| format!("{st:?}")),
            started_at_ns: card.started_at_ns,
            schema_version: card.schema_version,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectivityReportDto {
    pub endpoint: String,
    pub passed: bool,
    pub stages: Vec<StageResultDto>,
    pub rtt: Option<RttSummaryDto>,
}

/// Stable stage identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageDto {
    Dns,
    Tcp,
    Tls,
    Hello,
    Auth,
    SessionService,
    Rtt,
}

impl From<transport::connectivity::Stage> for StageDto {
    fn from(stage: transport::connectivity::Stage) -> Self {
        use transport::connectivity::Stage;
        match stage {
            Stage::Dns => StageDto::Dns,
            Stage::Tcp => StageDto::Tcp,
            Stage::Tls => StageDto::Tls,
            Stage::Hello => StageDto::Hello,
            Stage::Auth => StageDto::Auth,
            Stage::SessionService => StageDto::SessionService,
            Stage::Rtt => StageDto::Rtt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageOutcomeDto {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StageResultDto {
    pub stage: StageDto,
    pub stage_label: String,
    pub outcome: StageOutcomeDto,
    pub detail: Option<String>,
    pub error: Option<String>,
    pub duration_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RttSummaryDto {
    pub samples: u64,
    pub min_ms: f64,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub clock_offset_ns: Option<i64>,
    pub clock_uncertainty_ns: Option<u64>,
}

impl From<transport::connectivity::ConnectivityReport> for ConnectivityReportDto {
    fn from(report: transport::connectivity::ConnectivityReport) -> Self {
        use transport::connectivity::StageOutcome;
        let passed = report.passed();
        let stages = report
            .stages
            .into_iter()
            .map(|s| {
                let (outcome, detail, error) = match s.outcome {
                    StageOutcome::Passed { detail } => {
                        (StageOutcomeDto::Passed, Some(detail), None)
                    }
                    StageOutcome::Failed { error } => (StageOutcomeDto::Failed, None, Some(error)),
                    StageOutcome::Skipped => (StageOutcomeDto::Skipped, None, None),
                };
                StageResultDto {
                    stage: s.stage.into(),
                    stage_label: s.stage.label().to_owned(),
                    outcome,
                    detail,
                    error,
                    duration_ms: s.duration.as_secs_f64() * 1000.0,
                }
            })
            .collect();

        let rtt = report.rtt.map(|r| RttSummaryDto {
            samples: r.samples,
            min_ms: r.min.as_secs_f64() * 1000.0,
            avg_ms: r.avg.as_secs_f64() * 1000.0,
            max_ms: r.max.as_secs_f64() * 1000.0,
            clock_offset_ns: r.clock_offset_ns,
            clock_uncertainty_ns: r.clock_uncertainty_ns,
        });

        Self {
            endpoint: report.endpoint,
            passed,
            stages,
            rtt,
        }
    }
}

// ------------------------------------------------------------------------------------------------------------
// Helpers
// ------------------------------------------------------------------------------------------------------------

/// Client identifier sent in HELLO by one-shot connections. Deliberately constant: never the account name.
const ONE_SHOT_CLIENT_ID: &str = "desktop-oneshot";

fn active_role(state: &AppState) -> ClientRole {
    state.mode().map_or(ClientRole::Driver, ClientRole::from)
}

fn core_mode(mode: Mode) -> AppMode {
    match mode {
        Mode::Driver => AppMode::Driver,
        Mode::Engineer => AppMode::Engineer,
    }
}

fn require_relay(state: &AppState) -> Result<(), CommandError> {
    match state.relay_readiness() {
        RelayReadiness::Ready => Ok(()),
        RelayReadiness::NotConfigured => Err(CommandError::new(
            codes::RELAY_NOT_CONFIGURED,
            "No relay server is configured. Set it in the settings and restart the app.",
        )),
        RelayReadiness::RestartRequired => Err(CommandError::new(
            codes::RESTART_REQUIRED,
            "The relay settings changed. Restart the app to use them.",
        )),
    }
}

fn one_shot_config(core: &AppCore) -> transport::ClientConfig {
    transport::ClientConfig::new(
        core.endpoint().clone(),
        env!("CARGO_PKG_VERSION"),
        ONE_SHOT_CLIENT_ID,
        core.clock().epoch(),
    )
}

fn demo_running(core: &AppCore) -> bool {
    core.is_demo_running()
}

/// The live LMU driver pipeline must be the one receiving the command (never the display-only demo).
fn require_live_driver(core: &AppCore) -> Result<(), CommandError> {
    if demo_running(core) {
        return Err(CommandError::new(
            codes::DEMO_ACTIVE,
            "The demo is running. Stop the demo first.",
        ));
    }
    if core.mode() != AppMode::Driver {
        return Err(CoreError::InvalidMode(core.mode()).into());
    }
    if !core.is_driver_running() {
        return Err(CoreError::DriverNotRunning.into());
    }
    Ok(())
}

fn require_network(core: &AppCore) -> Result<(), CommandError> {
    require_live_driver(core)?;
    if !core.driver_network_attached() {
        return Err(CoreError::NetworkUnavailable.into());
    }
    Ok(())
}

fn require_recorder(core: &AppCore) -> Result<(), CommandError> {
    require_live_driver(core)?;
    if !core.driver_recorder_attached() {
        return Err(CoreError::RecorderUnavailable.into());
    }
    Ok(())
}

/// Status for `get_core_status`: the pipeline's latest `core:status` while that pipeline is the active one,
/// otherwise the core's own (idle) status. Never a stale status of a pipeline that stopped or of another origin.
fn current_status(core: &AppCore, cached: Option<CoreStatus>) -> CoreStatus {
    let base = core.status();
    let pipeline_active =
        base.mode == AppMode::Driver && (core.is_driver_running() || demo_running(core));
    match cached {
        Some(status)
            if pipeline_active && status.mode == base.mode && status.origin == base.origin =>
        {
            status
        }
        _ => base,
    }
}

/// Maximum accepted marker text, in characters.
pub const MAX_MARKER_CHARS: usize = 200;
/// Longest accepted username / display name, in characters (the relay enforces its own rules as well).
pub const MAX_NAME_CHARS: usize = 64;

fn non_empty_trimmed(field: &str, value: &str, max_chars: usize) -> Result<String, CommandError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CommandError::new(
            codes::INVALID_INPUT,
            format!("{field} must not be empty"),
        ));
    }
    if trimmed.chars().count() > max_chars {
        return Err(CommandError::new(
            codes::INVALID_INPUT,
            format!("{field} must be at most {max_chars} characters"),
        ));
    }
    Ok(trimmed.to_owned())
}

fn require_password(password: &Secret) -> Result<(), CommandError> {
    if password.expose().is_empty() {
        return Err(CommandError::new(
            codes::INVALID_INPUT,
            "password must not be empty",
        ));
    }
    Ok(())
}

/// Synthetic demo scenarios selectable from the UI. Fault-injection scenarios (stale, malformed) are test-only.
pub fn demo_scenario(name: Option<&str>) -> Result<Scenario, CommandError> {
    const SEED: u64 = 42;
    Ok(match name.unwrap_or("demo") {
        "demo" => Scenario::demo(SEED),
        "normal_lap" => Scenario::normal_lap(SEED),
        "pit_lap" => Scenario::pit_lap(SEED),
        "rain" => Scenario::rain(SEED),
        "hybrid" => Scenario::hybrid(SEED),
        "non_hybrid" => Scenario::non_hybrid(SEED),
        "safety_car" => Scenario::safety_car(SEED),
        other => {
            return Err(CommandError::new(
                codes::UNKNOWN_SCENARIO,
                format!(
                    "unknown demo scenario {other:?}; expected one of demo, normal_lap, pit_lap, rain, hybrid, \
                     non_hybrid, safety_car"
                ),
            ));
        }
    })
}

// ------------------------------------------------------------------------------------------------------------
// UI error log (rate-limited, bounded, redacted)
// ------------------------------------------------------------------------------------------------------------

/// Entries written per window; more are counted and reported with the next written entry.
pub const UI_ERROR_BURST: u32 = 10;
pub const UI_ERROR_WINDOW: Duration = Duration::from_secs(1);
pub const UI_ERROR_MAX_MESSAGE_CHARS: usize = 2_000;
pub const UI_ERROR_MAX_STACK_CHARS: usize = 8_000;
pub const UI_ERROR_MAX_COMPONENT_CHARS: usize = 100;

/// Fixed-window rate limiter for `log_ui_error`.
#[derive(Debug)]
pub struct UiErrorLimiter {
    window_start: Option<Instant>,
    in_window: u32,
    suppressed: u64,
}

impl UiErrorLimiter {
    pub const fn new() -> Self {
        Self {
            window_start: None,
            in_window: 0,
            suppressed: 0,
        }
    }

    /// `Some(suppressed_before)` if the entry may be written now, `None` if it is dropped (and counted).
    pub fn admit(&mut self, now: Instant) -> Option<u64> {
        let expired = self
            .window_start
            .is_none_or(|start| now.saturating_duration_since(start) >= UI_ERROR_WINDOW);
        if expired {
            self.window_start = Some(now);
            self.in_window = 0;
        }
        if self.in_window >= UI_ERROR_BURST {
            self.suppressed = self.suppressed.saturating_add(1);
            return None;
        }
        self.in_window += 1;
        Some(std::mem::take(&mut self.suppressed))
    }

    /// Entries dropped since the last admitted one.
    pub const fn suppressed(&self) -> u64 {
        self.suppressed
    }
}

impl Default for UiErrorLimiter {
    fn default() -> Self {
        Self::new()
    }
}

static UI_ERROR_LIMITER: Mutex<UiErrorLimiter> = Mutex::new(UiErrorLimiter::new());

/// Bound the length and mask anything that looks like a connection code (`XXXX-XXXX` / 8 code characters as a
/// whole word) or a bearer/token value. A second safety net: the UI must not put secrets in error text anyway.
pub fn redact_ui_text(text: &str, max_chars: usize) -> String {
    const SENSITIVE_KEYS: [&str; 6] = [
        "token",
        "password",
        "secret",
        "bearer",
        "authorization",
        "code",
    ];
    const REDACTED: &str = "[redacted]";
    let is_sensitive_key = |key: &str| {
        let key = key.to_ascii_lowercase();
        SENSITIVE_KEYS.iter().any(|k| key.ends_with(k))
    };

    let mut redacted = String::with_capacity(text.len());
    let mut mask_next = false;
    for piece in text.split_inclusive(char::is_whitespace) {
        let word = piece.trim_end_matches(char::is_whitespace);
        let separator = &piece[word.len()..];
        if word.is_empty() {
            redacted.push_str(separator);
            continue;
        }
        let bare = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
        if mask_next || looks_like_connect_code(bare) {
            redacted.push_str(REDACTED);
            mask_next = false;
        } else if let Some(split) = word.find(['=', ':']) {
            let (key, rest) = word.split_at(split);
            if !is_sensitive_key(key) {
                redacted.push_str(word);
            } else if rest.len() > 1 {
                // `key=value` / `key:value`: keep the key, mask the value.
                redacted.push_str(key);
                redacted.push_str(&rest[..1]);
                redacted.push_str(REDACTED);
            } else {
                // `key:` / `key=` followed by the value as the next word.
                redacted.push_str(word);
                mask_next = true;
            }
        } else {
            redacted.push_str(word);
        }
        redacted.push_str(separator);
    }

    let mut out = String::with_capacity(redacted.len().min(max_chars) + 3);
    for (count, c) in redacted.chars().enumerate() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        out.push(if c.is_control() && c != '\n' && c != '\t' {
            ' '
        } else {
            c
        });
    }
    out
}

/// Connection codes are 8 characters from the code alphabet, optionally written as `XXXX-XXXX`.
fn looks_like_connect_code(word: &str) -> bool {
    let compact: String = word.chars().filter(|c| *c != '-').collect();
    let dash_ok = !word.contains('-') || (word.len() == 9 && word.as_bytes().get(4) == Some(&b'-'));
    dash_ok
        && compact.len() == 8
        && compact
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && compact.chars().any(|c| c.is_ascii_digit())
}

// ------------------------------------------------------------------------------------------------------------
// General Commands
// ------------------------------------------------------------------------------------------------------------

/// Application name, version, build profile and build environment.
#[tauri::command(rename_all = "snake_case")]
pub fn get_app_info(state: State<'_, AppState>) -> AppInfo {
    state.app_info()
}

/// Current configuration, its path, and any start-up load issue.
#[tauri::command(rename_all = "snake_case")]
pub fn get_config(state: State<'_, AppState>) -> ConfigSnapshot {
    state.snapshot()
}

/// Persist the operating mode, then make it the core's active mode and start its services. The choice is saved
/// even if the services fail to start; that failure is returned.
#[tauri::command(async, rename_all = "snake_case")]
pub fn set_mode(state: State<'_, AppState>, mode: Mode) -> Result<ConfigSnapshot, CommandError> {
    let snapshot = state
        .set_mode(mode)
        .map_err(|e| logged("set_mode", e.into()))?;
    state
        .core()
        .activate_mode(core_mode(mode))
        .map_err(|e| logged("set_mode", e.into()))?;
    Ok(snapshot)
}

/// Validate and atomically save a full configuration, then apply it to the core (`apply_settings`). Relay
/// changes need a restart (`restart_required` in the result).
#[tauri::command(async, rename_all = "snake_case")]
pub fn update_settings(
    state: State<'_, AppState>,
    patch: AppConfig,
) -> Result<ConfigSnapshot, CommandError> {
    let (snapshot, core_settings) = state
        .update_settings(patch)
        .map_err(|e| logged("update_settings", e.into()))?;
    state
        .core()
        .apply_settings(core_settings)
        .map_err(|e| logged("update_settings", e.into()))?;
    Ok(snapshot)
}

/// Composite status: the active pipeline's latest `core:status`, else the core's idle status.
#[tauri::command(rename_all = "snake_case")]
pub fn get_core_status(state: State<'_, AppState>) -> CoreStatus {
    current_status(state.core(), state.sink().last_status())
}

/// Runs the 7-stage connectivity test against the configured relay.
#[tauri::command(rename_all = "snake_case")]
pub async fn test_relay_connection(
    state: State<'_, AppState>,
) -> Result<ConnectivityReportDto, CommandError> {
    require_relay(&state).map_err(|e| logged("test_relay_connection", e))?;
    let core = state.core();
    let config = one_shot_config(core);
    let transport = transport::TcpTlsTelemetryTransport::new();
    let report = run_connectivity_test(
        &transport,
        &config,
        core.auth_manager(),
        active_role(&state),
    )
    .await;
    Ok(ConnectivityReportDto::from(report))
}

/// Writes a frontend error to the app log (blueprint §9 item 15): rate-limited, bounded and redacted.
#[tauri::command(rename_all = "snake_case")]
pub fn log_ui_error(entry: UiErrorEntry) -> UiErrorLogged {
    let admitted = UI_ERROR_LIMITER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .admit(Instant::now());
    let Some(suppressed) = admitted else {
        let pending = UI_ERROR_LIMITER
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .suppressed();
        return UiErrorLogged {
            logged: false,
            suppressed: pending,
        };
    };
    let message = redact_ui_text(&entry.message, UI_ERROR_MAX_MESSAGE_CHARS);
    let stack = entry
        .stack
        .as_deref()
        .map(|s| redact_ui_text(s, UI_ERROR_MAX_STACK_CHARS));
    let ui_component = entry.component.as_deref().map_or_else(
        || "unknown".to_owned(),
        |c| redact_ui_text(c, UI_ERROR_MAX_COMPONENT_CHARS),
    );
    tracing::error!(
        component = "desktop.ui",
        ui_component = %ui_component,
        stack = stack.as_deref(),
        suppressed_before = suppressed,
        "{message}"
    );
    UiErrorLogged {
        logged: true,
        suppressed,
    }
}

// ------------------------------------------------------------------------------------------------------------
// Authentication Commands
// ------------------------------------------------------------------------------------------------------------

/// Logs in for the active mode's role. The password crosses IPC once, straight into a [`Secret`].
#[tauri::command(rename_all = "snake_case")]
pub async fn login(
    state: State<'_, AppState>,
    username: String,
    password: Secret,
) -> Result<AccountInfoDto, CommandError> {
    let username = non_empty_trimmed("username", &username, MAX_NAME_CHARS)?;
    require_password(&password)?;
    require_relay(&state).map_err(|e| logged("login", e))?;
    let core = state.core();
    let config = one_shot_config(core);
    let transport = transport::TcpTlsTelemetryTransport::new();
    let info = transport::login(
        &transport,
        &config,
        core.auth_manager(),
        active_role(&state),
        &username,
        password,
    )
    .await
    .map_err(|e| logged("login", e.into()))?;
    Ok(AccountInfoDto::from(info))
}

/// Registers a new account on the relay (and logs it in).
#[tauri::command(rename_all = "snake_case")]
pub async fn register_account(
    state: State<'_, AppState>,
    username: String,
    display_name: String,
    role: Mode,
    password: Secret,
) -> Result<AccountInfoDto, CommandError> {
    let username = non_empty_trimmed("username", &username, MAX_NAME_CHARS)?;
    let display_name = non_empty_trimmed("display name", &display_name, MAX_NAME_CHARS)?;
    require_password(&password)?;
    require_relay(&state).map_err(|e| logged("register_account", e))?;
    let core = state.core();
    let config = one_shot_config(core);
    let transport = transport::TcpTlsTelemetryTransport::new();
    let info = transport::register(
        &transport,
        &config,
        core.auth_manager(),
        role.into(),
        &username,
        password,
        &display_name,
    )
    .await
    .map_err(|e| logged("register_account", e.into()))?;
    Ok(AccountInfoDto::from(info))
}

/// Logs out: forgets the tokens and deletes the stored refresh tokens and driver session.
#[tauri::command(async, rename_all = "snake_case")]
pub fn logout(state: State<'_, AppState>) -> Result<(), CommandError> {
    state.core().auth_manager().logout().map_err(|e| {
        logged(
            "logout",
            CommandError::new(codes::CREDENTIAL_STORE_FAILED, e.to_string()),
        )
    })
}

/// Whether a refresh token exists for the active mode's role, plus the account if known in this run.
#[tauri::command(async, rename_all = "snake_case")]
pub fn get_login_state(state: State<'_, AppState>) -> Result<LoginStateDto, CommandError> {
    let role = active_role(&state);
    let auth = state.core().auth_manager();
    let authenticated = auth.is_logged_in(role).map_err(|e| {
        logged(
            "get_login_state",
            CommandError::new(codes::CREDENTIAL_STORE_FAILED, e.to_string()),
        )
    })?;
    Ok(LoginStateDto {
        authenticated,
        role: role.into(),
        account: auth.account().map(AccountInfoDto::from),
    })
}

// ------------------------------------------------------------------------------------------------------------
// Driver Mode Commands
// ------------------------------------------------------------------------------------------------------------

/// Begins live streaming to the relay. Requires the live driver pipeline with a relay connection.
#[tauri::command(async, rename_all = "snake_case")]
pub fn driver_start_streaming(state: State<'_, AppState>) -> Result<(), CommandError> {
    let core = state.core();
    require_network(core)
        .and_then(|()| core.driver_start_streaming().map_err(Into::into))
        .map_err(|e| logged("driver_start_streaming", e))
}

/// Pauses live streaming to the relay (the relay session is kept for resume).
#[tauri::command(async, rename_all = "snake_case")]
pub fn driver_stop_streaming(state: State<'_, AppState>) -> Result<(), CommandError> {
    let core = state.core();
    require_network(core)
        .and_then(|()| core.driver_stop_streaming().map_err(Into::into))
        .map_err(|e| logged("driver_stop_streaming", e))
}

/// Ends the current driver session (finalises the recording; ends the relay session if connected).
#[tauri::command(async, rename_all = "snake_case")]
pub fn driver_end_session(state: State<'_, AppState>) -> Result<(), CommandError> {
    let core = state.core();
    require_live_driver(core)
        .and_then(|()| core.driver_end_session().map_err(Into::into))
        .map_err(|e| logged("driver_end_session", e))
}

/// Requests a new one-time connection code (shown via `core:status`, never returned or logged here).
#[tauri::command(async, rename_all = "snake_case")]
pub fn driver_new_code(state: State<'_, AppState>) -> Result<(), CommandError> {
    let core = state.core();
    require_network(core)
        .and_then(|()| core.driver_request_connect_code().map_err(Into::into))
        .map_err(|e| logged("driver_new_code", e))
}

/// Kicks a connected engineer by connection id and frees the slot.
#[tauri::command(async, rename_all = "snake_case")]
pub fn driver_kick_engineer(
    state: State<'_, AppState>,
    connection_id: u64,
) -> Result<(), CommandError> {
    let core = state.core();
    require_network(core)
        .and_then(|()| core.driver_kick_engineer(connection_id).map_err(Into::into))
        .map_err(|e| logged("driver_kick_engineer", e))
}

/// Turns local disk recording on or off. Requires a pipeline that owns a recorder.
#[tauri::command(async, rename_all = "snake_case")]
pub fn driver_set_recording(state: State<'_, AppState>, enabled: bool) -> Result<(), CommandError> {
    let core = state.core();
    require_recorder(core)
        .and_then(|()| core.driver_set_recording(enabled).map_err(Into::into))
        .map_err(|e| logged("driver_set_recording", e))
}

/// Appends a bookmark marker to the active recording.
#[tauri::command(async, rename_all = "snake_case")]
pub fn driver_add_marker(state: State<'_, AppState>, text: String) -> Result<(), CommandError> {
    let text = non_empty_trimmed("marker text", &text, MAX_MARKER_CHARS)?;
    let core = state.core();
    require_recorder(core)
        .and_then(|()| core.driver_add_marker(&text).map_err(Into::into))
        .map_err(|e| logged("driver_add_marker", e))
}

/// Acknowledging alerts is not supported by the pipeline yet; reported as `not_implemented`, never faked.
#[tauri::command(rename_all = "snake_case")]
pub fn driver_ack_alert(id: String) -> Result<(), CommandError> {
    let _ = id;
    Err(logged(
        "driver_ack_alert",
        CommandError::new(
            codes::NOT_IMPLEMENTED,
            "Acknowledging alerts is not available yet.",
        ),
    ))
}

// ------------------------------------------------------------------------------------------------------------
// Engineer Mode Commands
// ------------------------------------------------------------------------------------------------------------

/// Redeems a one-time connection code typed by the user and returns the driver's identity card. The stream is
/// held until `engineer_confirm`. This is the ONLY command that joins a driver (D-019/D-022: never automatic).
/// The code is neither logged nor persisted.
#[tauri::command(async, rename_all = "snake_case")]
pub fn engineer_join(
    state: State<'_, AppState>,
    code: String,
) -> Result<IdentityCardDto, CommandError> {
    require_relay(&state).map_err(|e| logged("engineer_join", e))?;
    let card = state
        .core()
        .engineer_join_unconfirmed(&code)
        .map_err(|e| logged("engineer_join", e.into()))?;
    Ok(IdentityCardDto::from(card))
}

/// Accepts the identity card: the held stream (and everything after it) is delivered.
#[tauri::command(async, rename_all = "snake_case")]
pub fn engineer_confirm(state: State<'_, AppState>) -> Result<(), CommandError> {
    state
        .core()
        .engineer_confirm()
        .map_err(|e| logged("engineer_confirm", e.into()))
}

/// Declines the identity card; the code stays used (a new code is needed).
#[tauri::command(async, rename_all = "snake_case")]
pub fn engineer_cancel(state: State<'_, AppState>) -> Result<(), CommandError> {
    state
        .core()
        .engineer_cancel()
        .map_err(|e| logged("engineer_cancel", e.into()))
}

/// Disconnects from the driver; reconnecting needs a new code.
#[tauri::command(async, rename_all = "snake_case")]
pub fn engineer_disconnect(state: State<'_, AppState>) -> Result<(), CommandError> {
    let core = state.core();
    if core.engineer_client().is_none() {
        return Err(logged(
            "engineer_disconnect",
            CoreError::EngineerNotRunning.into(),
        ));
    }
    core.engineer_disconnect()
        .map_err(|e| logged("engineer_disconnect", e.into()))
}

/// Asks the driver for a fresh full snapshot.
#[tauri::command(async, rename_all = "snake_case")]
pub fn engineer_request_snapshot(state: State<'_, AppState>) -> Result<(), CommandError> {
    state
        .core()
        .engineer_request_snapshot()
        .map_err(|e| logged("engineer_request_snapshot", e.into()))
}

// ------------------------------------------------------------------------------------------------------------
// Demo Mode Commands
// ------------------------------------------------------------------------------------------------------------

/// Starts a synthetic demo scenario (default `demo`). Display-only: never recorded or streamed; `origin` is
/// `synthetic` in `core:status` and `bridge_subscribe`. Stops the live LMU pipeline while it runs.
#[tauri::command(async, rename_all = "snake_case")]
pub fn demo_start(
    state: State<'_, AppState>,
    scenario: Option<String>,
) -> Result<(), CommandError> {
    let scenario = demo_scenario(scenario.as_deref())?;
    state
        .core()
        .start_demo(scenario)
        .map_err(|e| logged("demo_start", e.into()))
}

/// Stops the demo and restarts the live LMU driver pipeline (so recording resumes).
#[tauri::command(async, rename_all = "snake_case")]
pub fn demo_stop(state: State<'_, AppState>) -> Result<(), CommandError> {
    let core = state.core();
    if !demo_running(core) {
        return Err(logged(
            "demo_stop",
            CommandError::new(codes::DEMO_ERROR, "the demo is not running"),
        ));
    }
    core.stop_demo()
        .map_err(|e| logged("demo_stop", e.into()))?;
    state.sink().clear_status();
    core.activate_mode(AppMode::Driver)
        .map_err(|e| logged("demo_stop", e.into()))
}

// ------------------------------------------------------------------------------------------------------------
// Bridge Commands
// ------------------------------------------------------------------------------------------------------------

/// Subscribes a WebView channel to the binary `UiBatch` stream. The first batch carries `RESET | BACKFILL`.
#[tauri::command(async, rename_all = "snake_case")]
pub fn bridge_subscribe(
    state: State<'_, AppState>,
    on_batch: Channel<InvokeResponseBody>,
) -> Result<BridgeSubscribeResponse, CommandError> {
    subscribe_channel(state.core(), on_batch)
}

fn subscribe_channel<C: crate::core_bridge::BatchChannel>(
    core: &AppCore,
    channel: C,
) -> Result<BridgeSubscribeResponse, CommandError> {
    let sink: Arc<dyn UiSink> = Arc::new(ChannelSink::new(channel));
    let hub = core.ui_hub();
    let sub = hub.subscribe(sink);
    if hub.subscription_dropped(sub).is_none() {
        return Err(logged(
            "bridge_subscribe",
            CommandError::new(
                codes::BRIDGE_CLOSED,
                "the batch channel closed during the initial backfill",
            ),
        ));
    }
    Ok(BridgeSubscribeResponse {
        subscription_id: sub.0,
        origin: core.status().origin,
    })
}

/// Removes a batch subscription. Idempotent: an unknown id is already unsubscribed.
#[tauri::command(rename_all = "snake_case")]
pub fn bridge_unsubscribe(state: State<'_, AppState>, subscription_id: u64) {
    state
        .core()
        .ui_hub()
        .unsubscribe(SubscriptionId(subscription_id));
}

// ------------------------------------------------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicBool, Ordering};

    use app_core::bridge::status::Origin;

    use super::*;
    use crate::config::{ConfigStore, Environment, FieldError};
    use crate::core_bridge::BatchChannel;
    use crate::state::tests::{identity, test_state};

    const BUILD_RS: &str = include_str!("../build.rs");
    const LIB_RS: &str = include_str!("lib.rs");
    const COMMANDS_RS: &str = include_str!("commands.rs");
    const CAPABILITY: &str = include_str!("../capabilities/default.json");
    const IPC_TS: &str = include_str!("../../src/lib/ipc.ts");

    /// Identifiers quoted with `quote` inside the first `[...]` after the `=` that follows `marker`.
    fn quoted_list(text: &str, marker: &str, quote: char) -> Vec<String> {
        let start = text
            .find(marker)
            .unwrap_or_else(|| panic!("{marker} not found"));
        let rest = &text[start..];
        let rest = &rest[rest.find('=').expect("=")..];
        let open = rest.find('[').expect("[");
        let close = rest[open..].find(']').expect("]") + open;
        rest[open..close]
            .split(quote)
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    fn set(items: impl IntoIterator<Item = String>) -> BTreeSet<String> {
        items.into_iter().collect()
    }

    #[test]
    fn command_lists_agree() {
        let expected: Vec<String> = COMMAND_NAMES.iter().map(|s| (*s).to_owned()).collect();
        let unique = set(expected.clone());
        assert_eq!(unique.len(), expected.len(), "duplicate command names");

        // build.rs COMMANDS (same order).
        assert_eq!(
            quoted_list(BUILD_RS, "const COMMANDS", '"'),
            expected,
            "build.rs COMMANDS"
        );

        // Capability allow-list: exactly one allow-<name> per command, nothing else app-specific.
        let capability: serde_json::Value =
            serde_json::from_str(CAPABILITY).expect("capability JSON");
        let permissions: Vec<String> = capability["permissions"]
            .as_array()
            .expect("permissions")
            .iter()
            .map(|p| p.as_str().expect("string permission").to_owned())
            .collect();
        let allowed = set(permissions
            .iter()
            .filter_map(|p| p.strip_prefix("allow-"))
            .map(|p| p.replace('-', "_")));
        assert_eq!(allowed, unique, "capabilities/default.json allow-list");
        for p in &permissions {
            assert!(
                p.starts_with("allow-") || p.starts_with("core:"),
                "unexpected permission {p}"
            );
        }

        // lib.rs generate_handler!
        let start = LIB_RS.find("generate_handler![").expect("generate_handler");
        let end = LIB_RS[start..].find(']').expect("]") + start;
        let handlers = set(LIB_RS[start..end].split("commands::").skip(1).map(|s| {
            s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .next()
                .unwrap_or("")
                .to_owned()
        }));
        assert_eq!(handlers, unique, "lib.rs generate_handler!");

        // Every #[tauri::command] function in this file.
        let mut defined = BTreeSet::new();
        let mut lines = COMMANDS_RS.lines().peekable();
        while let Some(line) = lines.next() {
            if line.trim_start().starts_with("#[tauri::command") {
                let def = lines.next().expect("fn after attribute");
                let name = def
                    .trim_start()
                    .trim_start_matches("pub ")
                    .trim_start_matches("async ")
                    .trim_start_matches("fn ")
                    .split(['(', '<'])
                    .next()
                    .expect("name")
                    .to_owned();
                defined.insert(name);
            }
        }
        assert_eq!(
            defined, unique,
            "#[tauri::command] functions in commands.rs"
        );

        // Frontend CommandName union.
        let ts_names = set(quoted_list(IPC_TS, "export const COMMAND_NAMES", '\''));
        assert_eq!(ts_names, unique, "src/lib/ipc.ts COMMAND_NAMES");

        // Generated permission files.
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("permissions/autogenerated");
        for name in &unique {
            assert!(
                dir.join(format!("{name}.toml")).is_file(),
                "missing permission file for {name}"
            );
        }
    }

    #[test]
    fn command_error_serializes_code_and_message() {
        let error = CommandError::from(ConfigError::Invalid(vec![FieldError {
            field: "network.relay_port".to_owned(),
            message: "must be between 1 and 65535 (got 0)".to_owned(),
        }]));
        let value = serde_json::to_value(&error).expect("serialize");
        assert_eq!(value["code"], "config_invalid");
        assert!(
            value["message"]
                .as_str()
                .is_some_and(|m| m.contains("network.relay_port"))
        );
    }

    #[test]
    fn core_and_join_errors_map_to_typed_codes() {
        let cases: Vec<(CoreError, &str)> = vec![
            (CoreError::ShutDown, codes::CORE_SHUTDOWN),
            (CoreError::DriverNotRunning, codes::DRIVER_NOT_RUNNING),
            (CoreError::RecorderUnavailable, codes::RECORDING_UNAVAILABLE),
            (CoreError::NetworkUnavailable, codes::NETWORK_UNAVAILABLE),
            (CoreError::Join(JoinError::CodeExpired), codes::CODE_EXPIRED),
            (
                CoreError::Join(JoinError::SessionFull { max: 2 }),
                codes::SESSION_FULL,
            ),
            (CoreError::Join(JoinError::Busy), codes::JOIN_BUSY),
            (
                CoreError::InvalidMode(AppMode::Engineer),
                codes::INVALID_MODE,
            ),
        ];
        for (error, code) in cases {
            assert_eq!(CommandError::from(error).code, code);
        }
        let full = CommandError::from(CoreError::Join(JoinError::SessionFull { max: 2 }));
        assert_eq!(full.message, "Driver already has 2 engineers connected");
        assert_eq!(
            CommandError::from(LoginError::InvalidCredentials).code,
            codes::INVALID_CREDENTIALS
        );
    }

    #[test]
    fn dto_enums_are_snake_case() {
        assert_eq!(
            serde_json::to_value(RoleDto::Engineer).expect("json"),
            "engineer"
        );
        assert_eq!(
            serde_json::to_value(StageDto::SessionService).expect("json"),
            "session_service"
        );
        assert_eq!(
            serde_json::to_value(StageOutcomeDto::Skipped).expect("json"),
            "skipped"
        );
    }

    #[test]
    fn passwords_are_secrets_and_never_printed() {
        let secret: Secret = serde_json::from_str("\"hunter2-pass\"").expect("deserialize");
        assert!(!format!("{secret:?}").contains("hunter2"));
        assert!(require_password(&Secret::new("")).is_err());
        assert!(require_password(&secret).is_ok());
        // Command arguments for login use `Secret` directly: no plain String password exists in this module.
        let login_sig = COMMANDS_RS
            .split("pub async fn login(")
            .nth(1)
            .and_then(|s| s.split(") ->").next())
            .expect("login signature");
        assert!(login_sig.contains("password: Secret"), "{login_sig}");
        let register_sig = COMMANDS_RS
            .split("pub async fn register_account(")
            .nth(1)
            .and_then(|s| s.split(") ->").next())
            .expect("register signature");
        assert!(register_sig.contains("password: Secret"), "{register_sig}");
    }

    #[test]
    fn only_engineer_join_joins_a_driver() {
        // D-019/D-022: no auto-connect. The adapter calls the core's join in exactly one place.
        let production = COMMANDS_RS
            .split("#[cfg(test)]")
            .next()
            .expect("non-test part");
        assert_eq!(production.matches(".engineer_join_unconfirmed(").count(), 1);
        assert_eq!(production.matches(".engineer_join(").count(), 0);
        let lib_rs = LIB_RS;
        assert!(
            !lib_rs.contains("engineer_join_unconfirmed(") && !lib_rs.contains(".engineer_join(")
        );
    }

    #[test]
    fn input_validation() {
        assert_eq!(non_empty_trimmed("x", "  ab ", 5).expect("ok"), "ab");
        assert_eq!(
            non_empty_trimmed("x", "   ", 5).expect_err("empty").code,
            codes::INVALID_INPUT
        );
        assert_eq!(
            non_empty_trimmed("x", "abcdef", 5).expect_err("long").code,
            codes::INVALID_INPUT
        );
        assert!(demo_scenario(None).is_ok());
        assert!(demo_scenario(Some("pit_lap")).is_ok());
        assert_eq!(
            demo_scenario(Some("malformed"))
                .expect_err("fault scenario")
                .code,
            codes::UNKNOWN_SCENARIO
        );
    }

    #[test]
    fn ack_alert_is_not_implemented_not_faked() {
        assert_eq!(
            driver_ack_alert("a1".to_owned())
                .expect_err("must not succeed")
                .code,
            codes::NOT_IMPLEMENTED
        );
    }

    #[test]
    fn ui_error_limiter_bounds_bursts_and_reports_suppressed() {
        let mut limiter = UiErrorLimiter::new();
        let t0 = Instant::now();
        for _ in 0..UI_ERROR_BURST {
            assert_eq!(limiter.admit(t0), Some(0));
        }
        assert_eq!(limiter.admit(t0), None);
        assert_eq!(limiter.admit(t0 + Duration::from_millis(500)), None);
        assert_eq!(limiter.suppressed(), 2);
        assert_eq!(limiter.admit(t0 + UI_ERROR_WINDOW), Some(2));
        assert_eq!(limiter.suppressed(), 0);
    }

    #[test]
    fn ui_error_text_is_redacted_and_bounded() {
        let text = redact_ui_text(
            "join failed for 7KQ4-MZ9T password=hunter2 token: abc.def ok",
            1000,
        );
        assert!(!text.contains("7KQ4"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("abc.def"), "{text}");
        assert!(
            text.contains("join failed for") && text.ends_with("ok"),
            "{text}"
        );
        assert!(!redact_ui_text("code 7KQ4MZ9T", 100).contains("7KQ4MZ9T"));
        assert_eq!(
            redact_ui_text("TypeError: x is undefined", 100),
            "TypeError: x is undefined"
        );
        let long = redact_ui_text(&"a".repeat(50), 10);
        assert_eq!(long.chars().count(), 11);
        assert!(!redact_ui_text("a\u{1b}[31mb", 100).contains('\u{1b}'));
    }

    struct TestChannel {
        closed: AtomicBool,
        delivered: Mutex<usize>,
    }

    impl BatchChannel for Arc<TestChannel> {
        fn deliver(&self, _batch: Vec<u8>) -> Result<(), String> {
            if self.closed.load(Ordering::SeqCst) {
                return Err("closed".to_owned());
            }
            *self.delivered.lock().expect("lock") += 1;
            Ok(())
        }
    }

    fn snapshot_block() -> Arc<[u8]> {
        let frame = telemetry_model::frame::TelemetryFrame::new(0, 1_000_000_000, Some(0.0));
        let mut buf = Vec::new();
        frame
            .encode_snapshot(&mut buf, telemetry_model::codec::RELAY_NS_UNKNOWN)
            .expect("encode");
        Arc::from(buf)
    }

    #[test]
    fn bridge_subscribe_returns_id_and_origin_and_reports_closed_channels() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ConfigStore::new(dir.path());
        let state = test_state(
            store.clone(),
            store.load(Environment::Development),
            Environment::Development,
            identity(),
        );
        let core = state.core();
        core.ui_hub().push_block(snapshot_block(), true);

        let channel = Arc::new(TestChannel {
            closed: AtomicBool::new(false),
            delivered: Mutex::new(0),
        });
        let response = subscribe_channel(core, Arc::clone(&channel)).expect("subscribe");
        assert_eq!(response.origin, Origin::Lmu);
        assert!(
            *channel.delivered.lock().expect("lock") >= 1,
            "backfill delivered"
        );
        let json = serde_json::to_value(&response).expect("json");
        assert_eq!(json["origin"], "lmu");
        assert!(json["subscription_id"].as_u64().is_some());
        assert_eq!(core.ui_hub().subscription_count(), 1);

        core.ui_hub()
            .unsubscribe(SubscriptionId(response.subscription_id));
        assert_eq!(core.ui_hub().subscription_count(), 0);

        let closed = Arc::new(TestChannel {
            closed: AtomicBool::new(true),
            delivered: Mutex::new(0),
        });
        let error = subscribe_channel(core, closed).expect_err("closed during backfill");
        assert_eq!(error.code, codes::BRIDGE_CLOSED);
        assert_eq!(core.ui_hub().subscription_count(), 0);
    }

    #[test]
    fn demo_origin_is_synthetic_and_driver_commands_refuse_the_demo() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ConfigStore::new(dir.path());
        let state = test_state(
            store.clone(),
            store.load(Environment::Development),
            Environment::Development,
            identity(),
        );
        let core = state.core();
        core.start_demo(demo_scenario(None).expect("scenario"))
            .expect("demo");
        let channel = Arc::new(TestChannel {
            closed: AtomicBool::new(false),
            delivered: Mutex::new(0),
        });
        let response = subscribe_channel(core, channel).expect("subscribe");
        assert_eq!(response.origin, Origin::Synthetic);
        assert_eq!(
            require_recorder(core).expect_err("demo").code,
            codes::DEMO_ACTIVE
        );
        assert_eq!(
            require_network(core).expect_err("demo").code,
            codes::DEMO_ACTIVE
        );
        assert_eq!(current_status(core, None).origin, Origin::Synthetic);
        core.stop_demo().expect("stop");
        assert_eq!(
            require_live_driver(core).expect_err("nothing runs").code,
            codes::DRIVER_NOT_RUNNING
        );
        core.shutdown().expect("shutdown");
    }

    #[test]
    fn cached_status_is_used_only_for_the_active_pipeline() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = ConfigStore::new(dir.path());
        let state = test_state(
            store.clone(),
            store.load(Environment::Development),
            Environment::Development,
            identity(),
        );
        let core = state.core();
        let mut cached = core.status();
        cached.recording.state = "active".to_owned();
        // No pipeline runs: the cached status of a stopped pipeline is not reported.
        assert_eq!(
            current_status(core, Some(cached.clone())).recording.state,
            "off"
        );
        core.start_demo(demo_scenario(None).expect("scenario"))
            .expect("demo");
        // Origin mismatch (cached LMU status while the synthetic demo runs) is not reported either.
        assert_eq!(
            current_status(core, Some(cached.clone())).origin,
            Origin::Synthetic
        );
        cached.origin = Origin::Synthetic;
        assert_eq!(current_status(core, Some(cached)).recording.state, "active");
        core.shutdown().expect("shutdown");
    }
}

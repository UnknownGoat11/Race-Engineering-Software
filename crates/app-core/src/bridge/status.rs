//! Serde DTOs for the JSON events sent to the UI (blueprint section 8 and 9).

use serde::{Deserialize, Serialize};

/// Mode of the desktop app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    Driver,
    Engineer,
}

/// Provenance of the active telemetry stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Live data acquired directly from LMU's shared memory.
    Lmu,
    /// Synthetic scenario telemetry for demonstration and offline testing.
    Synthetic,
}

/// LMU acquisition source status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LmuSourceStatus {
    pub state: String,
    pub detail: Option<String>,
    pub game_version: Option<u32>,
}

/// Driver session status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStatusDto {
    pub session_id: Option<u64>,
    pub session_type: Option<String>,
    pub track: Option<String>,
    pub car: Option<String>,
    pub vehicle_class: Option<String>,
    pub started_at_unix_s: Option<u64>,
}

/// Local recording status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingStatusDto {
    pub state: String,
    pub recording_id: Option<String>,
    pub bytes_written: u64,
    pub frames_written: u64,
    pub frames_lost: u64,
    pub disk_free_bytes: Option<u64>,
    pub error: Option<String>,
}

/// Network connectivity and session sharing status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkStatusDto {
    pub state: String,
    pub rtt_ms: Option<f32>,
    pub connect_code: Option<String>,
    pub code_expires_at_unix_s: Option<u64>,
    pub engineers_connected: u16,
    pub engineers_max: u16,
    pub degraded_reason: Option<String>,
    pub driver_away: bool,
    pub last_error: Option<String>,
    pub security_issue: bool,
}

/// Live throughput rates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatesDto {
    pub telemetry_read_hz: Option<f32>,
    pub scoring_read_hz: Option<f32>,
    pub frames_sent_hz: Option<f32>,
    pub bytes_sent_per_s: Option<u64>,
    pub frames_dropped_total: u64,
}

/// Drop and overrun counters.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DropsDto {
    pub reader_overrun: u64,
    pub pipeline_overrun: u64,
    pub recorder_overrun: u64,
    pub net_dropped_obsolete: u64,
    pub ui_dropped: u64,
}

/// 4 Hz composite status payload (`core:status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoreStatus {
    pub mode: AppMode,
    pub origin: Origin,
    pub lmu: LmuSourceStatus,
    pub session: SessionStatusDto,
    pub recording: RecordingStatusDto,
    pub network: NetworkStatusDto,
    pub rates: RatesDto,
    pub drops: DropsDto,
}

/// Alert severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Info,
    Warning,
    Critical,
}

/// Active or cleared alert notification (`core:alert`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertDto {
    pub id: String,
    pub name: String,
    pub level: AlertLevel,
    pub raised: bool,
    pub message: String,
    pub timestamp_ns: u64,
}

/// Diagnostic notice or operational error (`core:error`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDto {
    pub category: String,
    pub message: String,
    pub fatal: bool,
}

/// Engineer client phase and identity card (`core:engineer`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineerDto {
    pub phase: String,
    pub driver_name: Option<String>,
    pub car: Option<String>,
    pub vehicle_class: Option<String>,
    pub track: Option<String>,
    pub session_type: Option<String>,
    pub ended_reason: Option<String>,
}

/// Structured JSON events emitted across the bridge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", content = "data")]
pub enum UiEvent {
    #[serde(rename = "core:status")]
    Status(Box<CoreStatus>),
    #[serde(rename = "core:scoring")]
    Scoring(serde_json::Value),
    #[serde(rename = "core:session_event")]
    SessionEvent(serde_json::Value),
    #[serde(rename = "core:alert")]
    Alert(AlertDto),
    #[serde(rename = "core:capabilities")]
    Capabilities(serde_json::Value),
    #[serde(rename = "core:engine_state")]
    EngineState(serde_json::Value),
    #[serde(rename = "core:engineer")]
    Engineer(EngineerDto),
    #[serde(rename = "core:error")]
    Error(ErrorDto),
}

impl UiEvent {
    /// Canonical event name emitted across Tauri IPC.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::Status(_) => "core:status",
            Self::Scoring(_) => "core:scoring",
            Self::SessionEvent(_) => "core:session_event",
            Self::Alert(_) => "core:alert",
            Self::Capabilities(_) => "core:capabilities",
            Self::EngineState(_) => "core:engine_state",
            Self::Engineer(_) => "core:engineer",
            Self::Error(_) => "core:error",
        }
    }
}

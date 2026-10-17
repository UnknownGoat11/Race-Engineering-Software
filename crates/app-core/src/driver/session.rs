//! `DriverSessionMachine`: pure, table-driven driver session lifecycle state machine (blueprint §5, §12, §14).
//!
//! # Responsibilities
//! - Coordinates the three orthogonal axes feeding the UI (blueprint §5):
//!   1. **LMU Source State** ([`LmuSourceState`]): `NotRunning`, `Connecting`, `Live`, `Paused`, `Stale`, `Error`.
//!   2. **Recording State** ([`RecordingStatus`]): `Off`, `Active`, `Suspended`, `Failed`.
//!   3. **Network State** ([`ConnectionState`]): transport link status, plus connect code and engineer count.
//! - Enforces owner decisions (blueprint §14):
//!   - Q1: Synthetic/demo data is NEVER recorded or streamed.
//!   - Q3: LMU track or car change KEEPS the relay session (engineers stay attached); finishes old
//!     local recording and starts a new one; emits session events.
//!   - Q7: Auto-stream is OFF by default; recording is ALWAYS automatic and independent of network.
//!   - Q9: Recording runs independent of login and network.
//! - Pure and deterministic: takes plain value input events ([`SessionInputEvent`]) and returns
//!   explicit output actions ([`SessionAction`]). No OS threading, IO, or network sockets inside.

use lmu_reader::ReaderState;
use protocol::messages::LmuSourceState;
use protocol::version::PROTOCOL_MAJOR;
use recorder::SessionMeta;
use serde::{Deserialize, Serialize};
use telemetry_model::events::{SessionEvent, Severity};
use telemetry_model::scoring::SessionIdentity;
use transport::ConnectionState;

use crate::bridge::status::{LmuSourceStatus, Origin, SessionStatusDto};
use crate::driver::recorder_sup::RecordingStatus;

/// Lifecycle stage of the driver session machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriverSessionStage {
    /// Idle before acquisition begins or after an explicit end.
    Idle,
    /// Waiting for LMU process or shared memory mapping (`LMU_Data`).
    WaitingForLmu,
    /// Attached to LMU shared memory, waiting for the first consistent telemetry frame.
    Connecting,
    /// Telemetry frames are arriving, but session identity is incomplete or ET is not yet sane.
    TelemetryAvailable,
    /// Identity is complete (track, vehicle and session type known, ET sane). Local session ready.
    SessionReady,
    /// Live streaming to relay is active.
    Streaming,
    /// LMU acquisition has stalled (frames stopped arriving).
    Stalled,
    /// LMU shared memory interface or layout is incompatible.
    Incompatible,
}

impl DriverSessionStage {
    /// Canonical upper-case label for diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::WaitingForLmu => "WAITING_FOR_LMU",
            Self::Connecting => "CONNECTING",
            Self::TelemetryAvailable => "TELEMETRY_AVAILABLE",
            Self::SessionReady => "SESSION_READY",
            Self::Streaming => "STREAMING",
            Self::Stalled => "STALLED",
            Self::Incompatible => "INCOMPATIBLE",
        }
    }
}

/// Settings governing driver session behavior.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverSessionSettings {
    /// When true, automatically starts live streaming once telemetry is ready and login is valid.
    /// Default: `false` (owner decision Q7).
    pub auto_stream: bool,
    /// Data provenance (LMU live data or synthetic scenario).
    /// Default: `Origin::Lmu`.
    pub origin: Origin,
    /// Application version string included in recording headers and session negotiation.
    pub app_version: String,
    /// Schema version string or number.
    pub schema_version: u16,
}

impl Default for DriverSessionSettings {
    fn default() -> Self {
        Self {
            auto_stream: false,
            origin: Origin::Lmu,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
        }
    }
}

/// Discrete input events processed by [`DriverSessionMachine`].
#[derive(Debug, Clone, PartialEq)]
pub enum SessionInputEvent {
    /// LMU reader connection state changed.
    ReaderStateChanged {
        state: ReaderState,
        detail: Option<String>,
        game_version: Option<u32>,
    },
    /// A telemetry snapshot was received from LMU or the demo provider.
    FrameReceived {
        capture_mono_ns: u64,
        in_realtime: bool,
        et_s: f64,
        identity: SessionIdentity,
        session_id: Option<u64>,
    },
    /// Periodic housekeeping tick expired without receiving a frame (staleness detection).
    FrameTimeout { in_realtime: bool },
    /// Wall-clock vs monotonic drift detected (e.g. PC sleep/resume or clock jump > 5s).
    ClockSkewDetected { skew_s: f64 },
    /// Driver authentication and relay configuration status changed.
    AuthStatusChanged {
        logged_in: bool,
        relay_configured: bool,
    },
    /// Transport connection state changed.
    TransportStateChanged(ConnectionState),
    /// Recorder supervisor status updated.
    RecordingStatusChanged(RecordingStatus),
    /// Configuration or runtime settings updated.
    SettingsUpdated(DriverSessionSettings),
    /// User action: start streaming to relay.
    ManualStartStreaming,
    /// User action: stop streaming to relay (keeps session for resume).
    ManualStopStreaming,
    /// User action: explicitly end the session (clears relay session, disconnects engineers).
    ManualEndSession,
}

/// Discrete output actions produced by [`DriverSessionMachine`].
#[derive(Debug, Clone, PartialEq)]
pub enum SessionAction {
    /// Start a new local recording session with the provided metadata.
    StartRecording(SessionMeta),
    /// Finish the active recording segment (e.g. on LMU stall or exit).
    FinishRecordingSegment,
    /// Resume the active recording segment (e.g. on LMU frame resumption).
    ResumeRecordingSegment,
    /// Finalize and close the current local recording session.
    FinishRecordingSession,
    /// Request transport client to begin live streaming to the relay.
    StartStreaming,
    /// Request transport client to pause/stop streaming (session kept for resume).
    StopStreaming,
    /// Request transport client to close the relay session permanently.
    EndStreamingSession,
    /// Broadcast a stale frame block with `Quality::Stale` across pipeline fan-out.
    SendStaleBlock,
    /// Send `SCHEMA_NEGOTIATE` over the network stream (resets sequence space).
    SendSchemaNegotiate,
    /// Send a full `TELEMETRY_SNAPSHOT` over the network stream.
    SendSnapshot,
    /// Broadcast a UI `RESET` batch to clear or reprime frontend history rings.
    EmitUiReset,
    /// Broadcast a discrete [`SessionEvent`].
    EmitSessionEvent(SessionEvent),
    /// Signal a pipeline discontinuity (resets rate estimators and marks time regression).
    RecordDiscontinuity(String),
}

/// Canonical upper-case name of an LMU source state (`LmuSourceStatus::state`).
pub const fn lmu_source_state_name(state: LmuSourceState) -> &'static str {
    match state {
        LmuSourceState::NotRunning => "NOT_RUNNING",
        LmuSourceState::Connecting => "CONNECTING",
        LmuSourceState::Live => "LIVE",
        LmuSourceState::Paused => "PAUSED",
        LmuSourceState::Stale => "STALE",
        LmuSourceState::Error => "ERROR",
    }
}

/// `LmuSourceStatus::state` while the LMU source is not observed (no driver pipeline, or the
/// driver's status has not arrived yet): unknown, never a healthy value.
pub const LMU_STATE_UNKNOWN: &str = "UNKNOWN";

/// Helper function to check if a session identity is complete and has sane ET.
pub fn is_identity_complete(identity: &SessionIdentity, et_s: Option<f64>) -> bool {
    let track_ok = identity
        .track_name
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty());
    let vehicle_ok = identity
        .vehicle_name
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty())
        || identity
            .vehicle_model
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty());
    let session_type_ok = identity.session_type.is_some();
    let et_ok = et_s.is_some_and(|et| et.is_finite() && et >= 0.0);
    track_ok && vehicle_ok && session_type_ok && et_ok
}

/// Pure state machine coordinating driver session lifecycle.
#[derive(Debug, Clone)]
pub struct DriverSessionMachine {
    stage: DriverSessionStage,
    lmu_source_state: LmuSourceState,
    lmu_detail: Option<String>,
    game_version: Option<u32>,
    active_identity: Option<SessionIdentity>,
    active_session_id: Option<u64>,
    in_realtime: bool,
    last_et_s: Option<f64>,
    streaming: bool,
    logged_in: bool,
    relay_configured: bool,
    connection_state: ConnectionState,
    recording_status: RecordingStatus,
    settings: DriverSessionSettings,
    session_started_at_unix_s: Option<u64>,
}

impl DriverSessionMachine {
    /// Creates a new driver session machine with the given settings.
    pub fn new(settings: DriverSessionSettings) -> Self {
        Self {
            stage: DriverSessionStage::Idle,
            lmu_source_state: LmuSourceState::NotRunning,
            lmu_detail: None,
            game_version: None,
            active_identity: None,
            active_session_id: None,
            in_realtime: false,
            last_et_s: None,
            streaming: false,
            logged_in: false,
            relay_configured: false,
            connection_state: ConnectionState::Disconnected,
            recording_status: RecordingStatus::Off,
            settings,
            session_started_at_unix_s: None,
        }
    }

    /// Current lifecycle stage.
    pub fn stage(&self) -> DriverSessionStage {
        self.stage
    }

    /// Current LMU source state.
    pub fn lmu_source_state(&self) -> LmuSourceState {
        self.lmu_source_state
    }

    /// Current recording status.
    pub fn recording_status(&self) -> RecordingStatus {
        self.recording_status
    }

    /// Current transport connection state.
    pub fn connection_state(&self) -> ConnectionState {
        self.connection_state
    }

    /// Whether streaming to the relay is active.
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// Whether the driver is logged in (last [`SessionInputEvent::AuthStatusChanged`]).
    pub fn is_logged_in(&self) -> bool {
        self.logged_in
    }

    /// Whether a relay is configured (last [`SessionInputEvent::AuthStatusChanged`]).
    pub fn is_relay_configured(&self) -> bool {
        self.relay_configured
    }

    /// Last known LMU game version, if reported.
    pub fn game_version(&self) -> Option<u32> {
        self.game_version
    }

    /// Active session identity, if known.
    pub fn active_identity(&self) -> Option<&SessionIdentity> {
        self.active_identity.as_ref()
    }

    /// Active session id, if set.
    pub fn active_session_id(&self) -> Option<u64> {
        self.active_session_id
    }

    /// Current settings.
    pub fn settings(&self) -> &DriverSessionSettings {
        &self.settings
    }

    /// Builds a [`SessionMeta`] struct from a [`SessionIdentity`].
    pub fn build_session_meta(&self, identity: &SessionIdentity) -> SessionMeta {
        let vehicle = identity
            .vehicle_name
            .clone()
            .or_else(|| identity.vehicle_model.clone());
        SessionMeta {
            app_version: self.settings.app_version.clone(),
            telemetry_schema_version: self.settings.schema_version,
            protocol_version: Some(PROTOCOL_MAJOR as u16),
            lmu_version: self.game_version.map(|v| v.to_string()),
            interface_version: None,
            calculation_engine_version: None,
            vehicle,
            track: identity.track_name.clone(),
            session_type: identity.session_type.map(|st| st.to_string()),
            data_availability_json: None,
        }
    }

    /// Checks if auto-stream conditions are met.
    pub fn can_auto_stream(&self) -> bool {
        self.settings.auto_stream
            && self.logged_in
            && self.relay_configured
            && self.settings.origin != Origin::Synthetic
            && matches!(
                self.stage,
                DriverSessionStage::TelemetryAvailable | DriverSessionStage::SessionReady
            )
            && !self.streaming
    }

    /// Steps the state machine with an input event and returns an ordered list of output actions.
    pub fn step(&mut self, event: SessionInputEvent) -> Vec<SessionAction> {
        let mut actions = Vec::new();

        match event {
            SessionInputEvent::ReaderStateChanged {
                state,
                detail,
                game_version,
            } => {
                self.lmu_detail = detail.clone();
                if let Some(v) = game_version {
                    self.game_version = Some(v);
                }

                match state {
                    ReaderState::WaitingForLmu => {
                        if self.lmu_source_state != LmuSourceState::NotRunning {
                            self.lmu_source_state = LmuSourceState::NotRunning;
                            self.stage = DriverSessionStage::WaitingForLmu;
                            actions.push(SessionAction::SendStaleBlock);
                            actions.push(SessionAction::EmitSessionEvent(
                                SessionEvent::LmuDisconnected {
                                    reason: detail.unwrap_or_else(|| "Waiting for LMU".to_string()),
                                },
                            ));
                            actions.push(SessionAction::FinishRecordingSegment);
                            // Note: Relay session stays up if streaming so engineers stay attached.
                        }
                    }
                    ReaderState::Connected => {
                        if self.lmu_source_state == LmuSourceState::NotRunning
                            || self.stage == DriverSessionStage::WaitingForLmu
                            || self.stage == DriverSessionStage::Idle
                        {
                            self.lmu_source_state = LmuSourceState::Connecting;
                            self.stage = DriverSessionStage::Connecting;
                        }
                    }
                    ReaderState::Stalled => {
                        self.handle_stall(&mut actions);
                    }
                    ReaderState::Incompatible => {
                        self.lmu_source_state = LmuSourceState::Error;
                        self.stage = DriverSessionStage::Incompatible;
                        actions.push(SessionAction::SendStaleBlock);
                        actions.push(SessionAction::FinishRecordingSession);
                        if self.streaming {
                            self.streaming = false;
                            actions.push(SessionAction::StopStreaming);
                        }
                        actions.push(SessionAction::EmitSessionEvent(SessionEvent::AlertRaised {
                            alert_id: "lmu_incompatible".to_string(),
                            severity: Severity::Critical,
                            message: detail.unwrap_or_else(|| {
                                "LMU shared-memory layout is incompatible".to_string()
                            }),
                        }));
                    }
                }
            }

            SessionInputEvent::FrameReceived {
                capture_mono_ns: _capture_mono_ns,
                in_realtime,
                et_s,
                identity,
                session_id,
            } => {
                if let Some(sid) = session_id {
                    self.active_session_id = Some(sid);
                }

                // 1. Resumption check from stall or connecting
                if self.lmu_source_state == LmuSourceState::Paused {
                    actions.push(SessionAction::EmitSessionEvent(SessionEvent::LmuResumed));
                    actions.push(SessionAction::ResumeRecordingSegment);
                    if self.streaming {
                        actions.push(SessionAction::SendSchemaNegotiate);
                        actions.push(SessionAction::SendSnapshot);
                    }
                } else if self.lmu_source_state == LmuSourceState::Stale {
                    actions.push(SessionAction::EmitSessionEvent(
                        SessionEvent::LmuConnected {
                            game_version: self.game_version.map(|v| v as i32),
                        },
                    ));
                    actions.push(SessionAction::ResumeRecordingSegment);
                    if self.streaming {
                        actions.push(SessionAction::SendSchemaNegotiate);
                        actions.push(SessionAction::SendSnapshot);
                    }
                } else if self.lmu_source_state == LmuSourceState::NotRunning
                    || self.lmu_source_state == LmuSourceState::Connecting
                {
                    actions.push(SessionAction::EmitSessionEvent(
                        SessionEvent::LmuConnected {
                            game_version: self.game_version.map(|v| v as i32),
                        },
                    ));
                    if self.active_identity.is_some() {
                        actions.push(SessionAction::ResumeRecordingSegment);
                        if self.streaming {
                            actions.push(SessionAction::SendSchemaNegotiate);
                            actions.push(SessionAction::SendSnapshot);
                        }
                    }
                }

                self.lmu_source_state = LmuSourceState::Live;
                self.in_realtime = in_realtime;

                // 2. ET regression detection (e.g. session restart without track/car change)
                if let Some(last_et) = self
                    .last_et_s
                    .filter(|&last| et_s + 2.0 < last && et_s >= 0.0)
                {
                    actions.push(SessionAction::EmitUiReset);
                    actions.push(SessionAction::RecordDiscontinuity(format!(
                        "ET regression from {last_et:.3}s to {et_s:.3}s"
                    )));
                    if self.streaming {
                        actions.push(SessionAction::SendSchemaNegotiate);
                        actions.push(SessionAction::SendSnapshot);
                    }
                }
                self.last_et_s = Some(et_s);

                // 3. Identity tracking and change detection
                let id_complete = is_identity_complete(&identity, Some(et_s));

                if let Some(ref current_id) = self.active_identity {
                    let track_changed = current_id.track_name != identity.track_name
                        && identity.track_name.is_some();
                    let vehicle_changed = (current_id.vehicle_name != identity.vehicle_name
                        || current_id.class_name != identity.class_name)
                        && identity.vehicle_name.is_some();
                    let session_changed = current_id.session_type != identity.session_type
                        && identity.session_type.is_some();

                    if track_changed || vehicle_changed || session_changed {
                        // Per owner decision Q3: KEEP the relay session!
                        // Finish old local recording session and start a new one.
                        actions.push(SessionAction::FinishRecordingSession);

                        if track_changed {
                            actions.push(SessionAction::EmitSessionEvent(
                                SessionEvent::TrackChanged {
                                    track_name: identity.track_name.clone(),
                                },
                            ));
                        }
                        if vehicle_changed {
                            actions.push(SessionAction::EmitSessionEvent(
                                SessionEvent::VehicleChanged {
                                    vehicle_name: identity.vehicle_name.clone(),
                                    class_name: identity.class_name.clone(),
                                },
                            ));
                        }
                        if let Some(st) = identity.session_type.filter(|_| session_changed) {
                            actions.push(SessionAction::EmitSessionEvent(
                                SessionEvent::SessionChanged {
                                    from: current_id.session_type,
                                    to: st,
                                },
                            ));
                        }

                        if id_complete && self.settings.origin != Origin::Synthetic {
                            let meta = self.build_session_meta(&identity);
                            actions.push(SessionAction::StartRecording(meta));
                        }

                        if self.streaming {
                            actions.push(SessionAction::SendSchemaNegotiate);
                            actions.push(SessionAction::SendSnapshot);
                        }

                        self.active_identity = Some(identity.clone());
                    }
                } else {
                    // First identity observed
                    self.active_identity = Some(identity.clone());
                    if id_complete {
                        if self.stage != DriverSessionStage::Streaming {
                            self.stage = DriverSessionStage::SessionReady;
                        }
                        if self.settings.origin != Origin::Synthetic {
                            let meta = self.build_session_meta(&identity);
                            actions.push(SessionAction::StartRecording(meta));
                        }
                    } else if self.stage != DriverSessionStage::Streaming {
                        self.stage = DriverSessionStage::TelemetryAvailable;
                    }
                }

                if id_complete
                    && (self.stage == DriverSessionStage::TelemetryAvailable
                        || self.stage == DriverSessionStage::Stalled
                        || self.stage == DriverSessionStage::Connecting)
                {
                    if self.stage != DriverSessionStage::Streaming {
                        self.stage = DriverSessionStage::SessionReady;
                    }
                } else if !id_complete && self.stage == DriverSessionStage::Stalled {
                    self.stage = DriverSessionStage::TelemetryAvailable;
                }

                // 4. Auto-stream check
                if self.can_auto_stream() {
                    self.streaming = true;
                    self.stage = DriverSessionStage::Streaming;
                    actions.push(SessionAction::StartStreaming);
                }
            }

            SessionInputEvent::FrameTimeout { in_realtime } => {
                self.in_realtime = in_realtime;
                if self.lmu_source_state == LmuSourceState::Live {
                    self.handle_stall(&mut actions);
                }
            }

            SessionInputEvent::ClockSkewDetected { skew_s } => {
                actions.push(SessionAction::EmitUiReset);
                actions.push(SessionAction::RecordDiscontinuity(format!(
                    "Clock skew detected: {skew_s:.2}s"
                )));
                if self.streaming {
                    actions.push(SessionAction::SendSnapshot);
                }
                actions.push(SessionAction::EmitSessionEvent(
                    SessionEvent::AlertRaised {
                        alert_id: "clock_skew_detected".to_string(),
                        severity: Severity::Warning,
                        message: format!(
                            "System clock drift of {skew_s:.2}s detected; resynchronizing UI and streams"
                        ),
                    },
                ));
            }

            SessionInputEvent::AuthStatusChanged {
                logged_in,
                relay_configured,
            } => {
                self.logged_in = logged_in;
                self.relay_configured = relay_configured;
                if self.can_auto_stream() {
                    self.streaming = true;
                    self.stage = DriverSessionStage::Streaming;
                    actions.push(SessionAction::StartStreaming);
                }
            }

            SessionInputEvent::TransportStateChanged(state) => {
                self.connection_state = state;
                if state == ConnectionState::Failed && self.streaming {
                    self.streaming = false;
                    if self.stage == DriverSessionStage::Streaming {
                        self.stage = DriverSessionStage::SessionReady;
                    }
                }
            }

            SessionInputEvent::RecordingStatusChanged(status) => {
                self.recording_status = status;
            }

            SessionInputEvent::SettingsUpdated(settings) => {
                self.settings = settings;
                if self.can_auto_stream() {
                    self.streaming = true;
                    self.stage = DriverSessionStage::Streaming;
                    actions.push(SessionAction::StartStreaming);
                }
            }

            SessionInputEvent::ManualStartStreaming => {
                if !self.streaming
                    && self.settings.origin != Origin::Synthetic
                    && self.logged_in
                    && self.relay_configured
                {
                    self.streaming = true;
                    self.stage = DriverSessionStage::Streaming;
                    actions.push(SessionAction::StartStreaming);
                }
            }

            SessionInputEvent::ManualStopStreaming => {
                if self.streaming {
                    self.streaming = false;
                    if self.stage == DriverSessionStage::Streaming {
                        self.stage = DriverSessionStage::SessionReady;
                    }
                    actions.push(SessionAction::StopStreaming);
                    // Local recording continues uninterrupted (owner decision Q9).
                }
            }

            SessionInputEvent::ManualEndSession => {
                if self.streaming {
                    self.streaming = false;
                    actions.push(SessionAction::EndStreamingSession);
                }
                actions.push(SessionAction::FinishRecordingSession);
                self.stage = DriverSessionStage::Idle;
                self.active_identity = None;
                self.last_et_s = None;
            }
        }

        actions
    }

    fn handle_stall(&mut self, actions: &mut Vec<SessionAction>) {
        if self.stage != DriverSessionStage::Incompatible {
            self.stage = DriverSessionStage::Stalled;
        }

        if self.in_realtime {
            self.lmu_source_state = LmuSourceState::Stale;
            actions.push(SessionAction::SendStaleBlock);
            actions.push(SessionAction::EmitSessionEvent(
                SessionEvent::LmuDisconnected {
                    reason: "LMU telemetry stalled while in realtime".to_string(),
                },
            ));
            actions.push(SessionAction::FinishRecordingSegment);
        } else {
            self.lmu_source_state = LmuSourceState::Paused;
            actions.push(SessionAction::SendStaleBlock);
            actions.push(SessionAction::EmitSessionEvent(SessionEvent::LmuPaused));
            actions.push(SessionAction::FinishRecordingSegment);
        }
        // Note: Relay session stays up if streaming so engineers stay attached.
    }

    /// Constructs the current [`LmuSourceStatus`] for composite bridge status.
    pub fn lmu_source_status(&self) -> LmuSourceStatus {
        LmuSourceStatus {
            state: lmu_source_state_name(self.lmu_source_state).to_string(),
            detail: self.lmu_detail.clone(),
            game_version: self.game_version,
        }
    }

    /// Constructs the current [`SessionStatusDto`] for composite bridge status.
    pub fn session_status_dto(&self) -> SessionStatusDto {
        let (car, vehicle_class, track, session_type) = match self.active_identity {
            Some(ref id) => (
                id.vehicle_name.clone().or_else(|| id.vehicle_model.clone()),
                id.class_name
                    .clone()
                    .or_else(|| id.vehicle_class.map(|c| c.to_string())),
                id.track_name.clone(),
                id.session_type.map(|st| st.to_string()),
            ),
            None => (None, None, None, None),
        };

        SessionStatusDto {
            session_id: self.active_session_id,
            session_type,
            track,
            car,
            vehicle_class,
            started_at_unix_s: self.session_started_at_unix_s,
        }
    }
}

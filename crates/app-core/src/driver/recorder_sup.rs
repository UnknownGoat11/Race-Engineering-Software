//! Recorder supervisor: manages the local `RecordingStore`, session segmentation,
//! disk space probes, and failure-to-status mapping (blueprint §5, §10, §12 Lane E).
//!
//! Enforces:
//! - One `RecordingSession` per LMU session identity.
//! - Clean segmentation on LMU stall / restart / identity change.
//! - Non-blocking append fan-out (telemetry, scoring, weather, session events, markers).
//! - Writer state mapping to `RecordingStatus` (Active, Suspended, Failed, Off).
//! - Periodic event draining and alert generation (disk low/critical, overruns).
//! - Pluggable `DiskSpaceProbe` for testing simulated disk exhaustion and recovery.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use recorder::{
    AppendError, CaptureClock, DiskSpaceProbe, EventRecord, LapRecord, RecordKind, RecorderConfig,
    RecorderError, RecorderEvent, RecorderStats, RecordingSession, RecordingStore, SessionId,
    SessionMeta, SessionMetaUpdate, SessionOptions, StintRecord, SystemDiskSpace, WriterState,
};
use telemetry_model::events::{SessionEvent, Severity};

use crate::bridge::status::RecordingStatusDto;

/// Tracing component name.
const COMPONENT: &str = "app_core::driver::recorder_sup";

/// Local recording lifecycle status as seen by the UI and pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    /// Recording is disabled or no session is currently being recorded.
    Off,
    /// Actively writing records to disk segments.
    Active,
    /// Suspended due to low or critical disk space (records are dropped and counted).
    Suspended,
    /// Writer encountered an unrecoverable IO error; records are dropped until reset.
    Failed,
}

impl RecordingStatus {
    /// String representation matching `RecordingStatusDto::state`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Failed => "failed",
        }
    }
}

/// Simulated disk space probe for deterministic unit and integration tests.
#[derive(Debug, Clone)]
pub struct SimulatedDiskProbe {
    available: Arc<AtomicU64>,
}

impl SimulatedDiskProbe {
    /// Creates a probe initialized to `initial_bytes` available.
    pub fn new(initial_bytes: u64) -> Self {
        Self {
            available: Arc::new(AtomicU64::new(initial_bytes)),
        }
    }

    /// Sets the simulated free disk space.
    pub fn set_available_bytes(&self, bytes: u64) {
        self.available.store(bytes, Ordering::SeqCst);
    }

    /// Returns the currently configured free bytes.
    pub fn available_bytes_current(&self) -> u64 {
        self.available.load(Ordering::SeqCst)
    }
}

impl DiskSpaceProbe for SimulatedDiskProbe {
    fn available_bytes(&self, _path: &Path) -> std::io::Result<u64> {
        Ok(self.available.load(Ordering::SeqCst))
    }
}

/// Dynamic wrapper allowing a boxed `DiskSpaceProbe` to be passed into `SessionOptions::with_disk_probe`.
pub struct BoxedDiskProbe(pub Box<dyn DiskSpaceProbe>);

impl DiskSpaceProbe for BoxedDiskProbe {
    fn available_bytes(&self, path: &Path) -> std::io::Result<u64> {
        self.0.available_bytes(path)
    }
}

/// Pluggable factory for creating `DiskSpaceProbe` instances on each session start.
pub type DiskProbeFactory = Arc<dyn Fn() -> Box<dyn DiskSpaceProbe> + Send + Sync>;

/// Configuration for the [`RecorderSupervisor`].
#[derive(Debug, Clone)]
pub struct RecorderSupervisorConfig {
    /// Recorder storage and segmentation configuration.
    pub recorder: RecorderConfig,
    /// Whether recording is initially enabled.
    pub enabled: bool,
}

impl Default for RecorderSupervisorConfig {
    fn default() -> Self {
        Self {
            recorder: RecorderConfig::new("recordings"),
            enabled: true,
        }
    }
}

/// Outcome of periodically polling recorder events and statistics.
#[derive(Debug, Clone)]
pub struct RecorderPollOutcome {
    /// Raw events emitted by the writer thread during this interval.
    pub raw_events: Vec<RecorderEvent>,
    /// High-level session events translated from writer lifecycle events.
    pub session_events: Vec<SessionEvent>,
    /// Current recording lifecycle status.
    pub status: RecordingStatus,
    /// Current writer stats snapshot, if a session is active.
    pub stats: Option<RecorderStats>,
}

/// Supervises local disk recording sessions, lifecycle transitions, and diagnostic metrics.
pub struct RecorderSupervisor {
    store: Arc<RecordingStore>,
    active_session: Option<RecordingSession>,
    current_identity: Option<SessionMeta>,
    probe_factory: DiskProbeFactory,
    capture_clock: Option<Arc<dyn CaptureClock>>,
    enabled: bool,
    status: RecordingStatus,
    last_error: Option<String>,
    total_bytes_written: u64,
    total_frames_written: u64,
    total_frames_lost: u64,
}

impl RecorderSupervisor {
    /// Opens the recording store at the directory specified in `config`.
    pub fn open(
        config: RecorderSupervisorConfig,
        probe_factory: Option<DiskProbeFactory>,
        capture_clock: Option<Arc<dyn CaptureClock>>,
    ) -> Result<Self, RecorderError> {
        let store = Arc::new(RecordingStore::open(config.recorder)?);
        let factory = probe_factory
            .unwrap_or_else(|| Arc::new(|| Box::new(SystemDiskSpace) as Box<dyn DiskSpaceProbe>));

        info!(
            component = COMPONENT,
            dir = %store.config().recordings_dir.display(),
            enabled = config.enabled,
            "recorder supervisor initialized"
        );

        Ok(Self {
            store,
            active_session: None,
            current_identity: None,
            probe_factory: factory,
            capture_clock,
            enabled: config.enabled,
            status: RecordingStatus::Off,
            last_error: None,
            total_bytes_written: 0,
            total_frames_written: 0,
            total_frames_lost: 0,
        })
    }

    /// Access to the underlying [`RecordingStore`].
    pub fn store(&self) -> &Arc<RecordingStore> {
        &self.store
    }

    /// Returns the current recording status.
    pub fn status(&self) -> RecordingStatus {
        self.status
    }

    /// Whether recording is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the session ID of the currently active recording session, if any.
    pub fn active_session_id(&self) -> Option<SessionId> {
        self.active_session.as_ref().map(|s| s.id())
    }

    /// Returns the absolute path of the currently active recording directory, if any.
    pub fn active_session_dir(&self) -> Option<PathBuf> {
        self.active_session.as_ref().map(|s| s.dir().to_path_buf())
    }

    /// Driver manual control: enables or disables recording.
    ///
    /// When disabled, finishes any active session. When enabled, starts a session if an identity is known.
    pub fn set_enabled(&mut self, enabled: bool) -> Result<Vec<SessionEvent>, RecorderError> {
        if self.enabled == enabled {
            return Ok(Vec::new());
        }
        info!(
            component = COMPONENT,
            enabled, "recording enabled status toggled"
        );
        self.enabled = enabled;

        if !enabled {
            Ok(self.on_session_stalled())
        } else if let Some(meta) = self.current_identity.clone() {
            self.on_session_ready(meta)
        } else {
            Ok(Vec::new())
        }
    }

    /// Notifies the supervisor that LMU session identity is established or changed.
    ///
    /// Starts a new recording session if enabled and not already running for this identity.
    /// If an existing session was running for a different identity, finishes the old one first.
    pub fn on_session_ready(
        &mut self,
        meta: SessionMeta,
    ) -> Result<Vec<SessionEvent>, RecorderError> {
        let mut events = Vec::new();

        // Check if an active session already matches this identity:
        if let Some(ref current) = self.current_identity
            && current == &meta
            && self.active_session.is_some()
        {
            debug!(
                component = COMPONENT,
                "session identity unchanged; keeping active session"
            );
            return Ok(events);
        }

        // Finish prior session if one was active:
        if let Some(old) = self.active_session.take() {
            let id_str = old.id().to_string();
            match old.finish() {
                Ok(summary) => {
                    events.push(SessionEvent::RecordingStopped {
                        recording_id: id_str,
                        frames: summary.stats.records_written,
                        bytes: summary.stats.bytes_written,
                    });
                    self.total_bytes_written = self
                        .total_bytes_written
                        .saturating_add(summary.stats.bytes_written);
                    self.total_frames_written = self
                        .total_frames_written
                        .saturating_add(summary.stats.records_written);
                }
                Err(err) => {
                    warn!(component = COMPONENT, error = %err, "error finishing prior session on identity change");
                    events.push(SessionEvent::RecordingFailed {
                        recording_id: Some(id_str),
                        reason: err.to_string(),
                    });
                }
            }
        }

        self.current_identity = Some(meta.clone());

        if !self.enabled {
            self.status = RecordingStatus::Off;
            debug!(
                component = COMPONENT,
                "recording disabled; not starting session for identity"
            );
            return Ok(events);
        }

        let mut options =
            SessionOptions::new().with_disk_probe(BoxedDiskProbe((self.probe_factory)()));
        if let Some(ref clock) = self.capture_clock {
            options = options.with_capture_clock(Arc::clone(clock));
        }

        match self.store.start_session(meta, options) {
            Ok(session) => {
                let id_str = session.id().to_string();
                info!(component = COMPONENT, session_id = %id_str, "recording session started");
                self.active_session = Some(session);
                self.status = RecordingStatus::Active;
                self.last_error = None;

                events.push(SessionEvent::RecordingStarted {
                    recording_id: id_str,
                });
                Ok(events)
            }
            Err(err) => {
                error!(component = COMPONENT, error = %err, "failed to start recording session");
                self.status = RecordingStatus::Failed;
                self.last_error = Some(err.to_string());

                events.push(SessionEvent::RecordingFailed {
                    recording_id: None,
                    reason: err.to_string(),
                });
                Err(err)
            }
        }
    }

    /// Notifies the supervisor that LMU telemetry stalled or was lost.
    ///
    /// Finishes the active recording segment and marks status `Off`.
    pub fn on_session_stalled(&mut self) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        if let Some(session) = self.active_session.take() {
            let id_str = session.id().to_string();
            match session.finish() {
                Ok(summary) => {
                    info!(
                        component = COMPONENT,
                        session_id = %id_str,
                        records = summary.stats.records_written,
                        bytes = summary.stats.bytes_written,
                        "recording session finalized on stall"
                    );
                    events.push(SessionEvent::RecordingStopped {
                        recording_id: id_str,
                        frames: summary.stats.records_written,
                        bytes: summary.stats.bytes_written,
                    });
                    self.total_bytes_written = self
                        .total_bytes_written
                        .saturating_add(summary.stats.bytes_written);
                    self.total_frames_written = self
                        .total_frames_written
                        .saturating_add(summary.stats.records_written);
                }
                Err(err) => {
                    warn!(component = COMPONENT, error = %err, "error finalizing recording session on stall");
                    events.push(SessionEvent::RecordingFailed {
                        recording_id: Some(id_str),
                        reason: err.to_string(),
                    });
                }
            }
        }
        self.status = RecordingStatus::Off;
        events
    }

    /// Explicitly ends the session (e.g. driver manually clicks end session).
    pub fn on_session_end(&mut self) -> Vec<SessionEvent> {
        self.current_identity = None;
        self.on_session_stalled()
    }

    /// Appends canonical TLV telemetry for the current frame.
    ///
    /// If recording is not currently active, silently ignores the call without error.
    pub fn append_telemetry(&self, capture_mono_ns: u64, bytes: &[u8]) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.append(RecordKind::TelemetryTlv, capture_mono_ns, bytes)
        } else {
            Ok(())
        }
    }

    /// Appends postcard-encoded scoring data.
    pub fn append_scoring(&self, capture_mono_ns: u64, bytes: &[u8]) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.append(RecordKind::ScoringPostcard, capture_mono_ns, bytes)
        } else {
            Ok(())
        }
    }

    /// Appends weather channels TLV payload.
    pub fn append_weather(&self, capture_mono_ns: u64, bytes: &[u8]) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.append(RecordKind::WeatherTlv, capture_mono_ns, bytes)
        } else {
            Ok(())
        }
    }

    /// Appends a session event record.
    pub fn append_session_event(
        &self,
        capture_mono_ns: u64,
        bytes: &[u8],
    ) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.append(RecordKind::SessionEvent, capture_mono_ns, bytes)
        } else {
            Ok(())
        }
    }

    /// Appends an application marker (bookmark/annotation).
    pub fn append_marker(&self, capture_mono_ns: u64, text: &str) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.append(RecordKind::Marker, capture_mono_ns, text.as_bytes())
        } else {
            Ok(())
        }
    }

    /// Appends a schema negotiation or capability map record.
    pub fn append_capability(&self, capture_mono_ns: u64, bytes: &[u8]) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.append(RecordKind::CapabilityPostcard, capture_mono_ns, bytes)
        } else {
            Ok(())
        }
    }

    /// Indexes a lap record into the session index.
    pub fn record_lap(&self, lap: LapRecord) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.record_lap(lap)
        } else {
            Ok(())
        }
    }

    /// Indexes a stint record into the session index.
    pub fn record_stint(&self, stint: StintRecord) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.record_stint(stint)
        } else {
            Ok(())
        }
    }

    /// Indexes an event record into the session index.
    pub fn record_event(&self, event: EventRecord) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.record_event(event)
        } else {
            Ok(())
        }
    }

    /// Updates session metadata in the index.
    pub fn update_meta(&self, update: SessionMetaUpdate) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.update_meta(update)
        } else {
            Ok(())
        }
    }

    /// Requests segment rotation on the active session (e.g. after time regression).
    pub fn request_rotation(&self) -> Result<(), AppendError> {
        if let Some(ref session) = self.active_session {
            session.request_rotation()
        } else {
            Ok(())
        }
    }

    /// Synchronizes all buffered records to disk durability.
    pub fn sync(&self, timeout: std::time::Duration) -> Result<(), RecorderError> {
        if let Some(ref session) = self.active_session {
            let _report = session.sync(timeout)?;
            Ok(())
        } else {
            Ok(())
        }
    }

    /// Periodically drains writer events and updates status and diagnostic counters.
    ///
    /// Should be called on the 10 Hz housekeeping tick.
    pub fn poll_events(&mut self) -> RecorderPollOutcome {
        let mut raw_events = Vec::new();
        let mut session_events = Vec::new();

        let stats = if let Some(ref session) = self.active_session {
            while let Ok(ev) = session.events().try_recv() {
                match &ev {
                    RecorderEvent::RecordingFailed { error, .. } => {
                        session_events.push(SessionEvent::RecordingFailed {
                            recording_id: Some(session.id().to_string()),
                            reason: error.clone(),
                        });
                    }
                    RecorderEvent::DiskSpaceCritical {
                        available_bytes,
                        threshold_bytes,
                    } => {
                        session_events.push(SessionEvent::AlertRaised {
                            alert_id: "disk_space_critical".to_string(),
                            severity: Severity::Critical,
                            message: format!(
                                "Disk space critically low: {available_bytes} bytes free (threshold: {threshold_bytes})"
                            ),
                        });
                    }
                    RecorderEvent::DiskSpaceLow {
                        available_bytes,
                        threshold_bytes,
                    } => {
                        session_events.push(SessionEvent::AlertRaised {
                            alert_id: "disk_space_low".to_string(),
                            severity: Severity::Warning,
                            message: format!(
                                "Disk space low: {available_bytes} bytes free (threshold: {threshold_bytes})"
                            ),
                        });
                    }
                    RecorderEvent::DiskSpaceRecovered { .. } => {
                        session_events.push(SessionEvent::AlertCleared {
                            alert_id: "disk_space_critical".to_string(),
                        });
                        session_events.push(SessionEvent::AlertCleared {
                            alert_id: "disk_space_low".to_string(),
                        });
                    }
                    _ => {}
                }
                raw_events.push(ev);
            }

            let st = session.stats();
            match st.state {
                WriterState::Recording => {
                    self.status = RecordingStatus::Active;
                }
                WriterState::Suspended => {
                    self.status = RecordingStatus::Suspended;
                }
                WriterState::Failed => {
                    self.status = RecordingStatus::Failed;
                }
                WriterState::Finished => {
                    self.status = RecordingStatus::Off;
                }
            }
            if st.last_error.is_some() {
                self.last_error = st.last_error.clone();
            }
            Some(st)
        } else {
            None
        };

        RecorderPollOutcome {
            raw_events,
            session_events,
            status: self.status,
            stats,
        }
    }

    /// Constructs the current [`RecordingStatusDto`] for bridge status emission.
    pub fn status_dto(&self) -> RecordingStatusDto {
        let stats = self.active_session.as_ref().map(|s| s.stats());
        let recording_id = self.active_session.as_ref().map(|s| s.id().to_string());

        let (bytes_written, frames_written, frames_lost, disk_free_bytes, error) = match stats {
            Some(ref st) => (
                self.total_bytes_written.saturating_add(st.bytes_written),
                self.total_frames_written.saturating_add(st.records_written),
                self.total_frames_lost.saturating_add(st.records_lost()),
                st.disk_free_bytes,
                st.last_error.clone().or_else(|| self.last_error.clone()),
            ),
            None => (
                self.total_bytes_written,
                self.total_frames_written,
                self.total_frames_lost,
                None,
                self.last_error.clone(),
            ),
        };

        RecordingStatusDto {
            state: self.status.as_str().to_string(),
            recording_id,
            bytes_written,
            frames_written,
            frames_lost,
            disk_free_bytes,
            error,
        }
    }
}

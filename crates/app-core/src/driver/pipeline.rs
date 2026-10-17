//! The pipeline thread (`"pipeline"`, NORMAL priority).
//!
//! Owns the `Normalizer`. Input: `PipelineInput::{Lmu(Box<RawSnapshot>), Demo(Box<SyntheticTick>),
//! Command(PipelineCommand), Tick}`. Per frame it runs normalization, the [`super::engines`] `EngineHub`,
//! the app channels (`SRC_*`, `PIPELINE_LATENCY`, `net.*`, `rec.*`) and the `AlertEngine`, then encodes one
//! canonical TLV block shared as `Arc<[u8]>` and fans it out without blocking. Event-driven: no timer sits
//! in the 100 Hz path. The 10 Hz housekeeping tick (source-age ticker, alert clock, recorder event drain,
//! sleep/resume check, status, relay state) runs every 100 ms whether or not input keeps arriving.
//!
//! Implemented in app-core Phase 2 Increment I-3 (blueprint §2, §3, §4, §5, §6, §7, §12).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use crossbeam_channel::{Receiver, Sender};
use lmu_normalizer::{Normalizer, TelemetryDisposition};
use lmu_reader::{RawSnapshot, ReaderDiagnostics, ReaderState, ReaderStats};
use parking_lot::Mutex;
use protocol::messages::{
    ClockSummary, ComponentHealth, DriverRates, DriverStatus, HealthLevel, LmuSourceState,
    RecordingState, RecordingStatus as WireRecordingStatus, SchemaNegotiate, SessionCreate,
};
use race_engines::alert::{AlertClock, AlertNotice, AlertState};
use recorder::LapRecord;
use telemetry_model::channel::FrequencyClass;
use telemetry_model::codec::{RELAY_NS_UNKNOWN, TlvEncoder, TlvHeader};
use telemetry_model::events::{EventRecord, EventTime, SessionEvent, Severity};
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::registry;
use telemetry_model::scoring::{ScoringState, SessionIdentity};
use telemetry_model::value::{Sample, Value};
use telemetry_model::{TELEMETRY_SCHEMA_VERSION, ch};
use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::bridge::hub::UiHub;
use crate::bridge::sink::UiSink;
use crate::bridge::status::{
    AlertDto, AlertLevel, AppMode, CoreStatus, DropsDto, ErrorDto, Origin, RatesDto,
    RecordingStatusDto, UiEvent,
};
use crate::clock::{AppClock, SkewStatus, SkewTracker};
use crate::driver::engines::{EngineHub, EngineHubConfig};
use crate::driver::link::network_status_dto;
use crate::driver::net::{LinkSnapshot, NetPublisher, StreamControlError};
use crate::driver::reader_thread::ReaderSnapshotMessage;
use crate::driver::recorder_sup::{RecorderSupervisor, RecordingStatus};
use crate::driver::session::{
    DriverSessionMachine, DriverSessionSettings, SessionAction, SessionInputEvent,
};

/// Tracing component name for logging.
const COMPONENT: &str = "app_core::driver::pipeline";

/// Housekeeping interval (10 Hz): status, stall detection, idle alerts, recorder drain.
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_millis(100);

/// Interval of the DRIVER_STATUS / RECORDING_STATUS publication while streaming (1 Hz).
const NET_STATUS_INTERVAL_NS: u64 = 1_000_000_000;

/// Shared slot holding the pipeline's latest composite status (read by `AppCore::status`).
pub type StatusSlot = Arc<Mutex<Option<CoreStatus>>>;

/// Shared reader diagnostics published by the reader thread (4 Hz).
pub type ReaderDiagnosticsSource = Arc<ArcSwap<Option<ReaderDiagnostics>>>;

/// Why the pipeline refused a command. Commands are never ignored silently: a refused command is
/// returned to the caller ([`PipelineCommand::Request`]) or reported as a `core:error` event.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandRejected {
    /// The pipeline has no recorder (recording disabled, store failed to open, or demo).
    #[error("local recording is not available")]
    RecorderUnavailable,
    /// A marker was requested while no recording session is open.
    #[error("no recording session is open (the LMU session is not ready yet)")]
    NotRecording,
    /// The pipeline has no relay connection (no relay configured, or the display-only demo).
    #[error("the driver is not connected to a relay")]
    NetworkUnavailable,
    /// Synthetic demo data is never streamed (owner decision Q1).
    #[error("synthetic demo data is never streamed")]
    SyntheticNeverStreamed,
    /// Streaming needs a driver login.
    #[error("log in as a driver to stream")]
    NotLoggedIn,
    /// No active alert has this id.
    #[error("no active alert with id {0}")]
    UnknownAlert(u64),
    /// The recorder refused the request.
    #[error("recorder error: {0}")]
    Recorder(String),
    /// The relay control queue is full or closed.
    #[error("relay control unavailable: {0}")]
    Control(StreamControlError),
}

/// Errors originating from the telemetry pipeline.
#[derive(Debug, Error)]
pub enum PipelineError {
    /// Frame validation or channel assignment error.
    #[error("frame error: {0}")]
    Frame(#[from] telemetry_model::frame::FrameError),

    /// TLV encoding error.
    #[error("codec error: {0}")]
    Codec(#[from] telemetry_model::codec::CodecError),

    /// Pipeline channel closed or broken.
    #[error("pipeline channel error: {0}")]
    Channel(String),

    /// Serialization error for events or scoring.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// A command was refused (typed reason).
    #[error("command refused: {0}")]
    Rejected(#[from] CommandRejected),
}

/// Commands accepted by the pipeline thread over its control queue.
#[derive(Debug, Clone)]
pub enum PipelineCommand {
    /// Requests an immediate full snapshot over UI and network streams.
    SnapshotNeeded,
    /// Requests driver client to begin live streaming to the cloud relay.
    StartStreaming,
    /// Requests driver client to pause/stop live streaming (session kept for resume).
    StopStreaming,
    /// Explicitly ends the active driver session, finalizing recordings and disconnecting.
    EndSession,
    /// Toggles local recording on or off.
    SetRecording(bool),
    /// Appends an annotation / bookmark marker to the active recording.
    AddMarker(String),
    /// Updates driver session settings (auto-stream, origin, etc.).
    UpdateSettings(DriverSessionSettings),
    /// Acknowledges an active alert of the pipeline's `AlertEngine` by id.
    AckAlert(u64),
    /// The driver login state changed (gates manual and automatic streaming).
    AuthStatus {
        /// A driver refresh token exists.
        logged_in: bool,
    },
    /// The relay bound (created or resumed) the session, or dropped the schema: send
    /// SCHEMA_NEGOTIATE and a full snapshot so the sequence space starts correctly.
    NetSessionReady,
    /// A non-fatal relay notice for the UI (`core:error`, not fatal).
    NetNotice {
        /// `core:error` category.
        category: String,
        /// User-facing text (never contains codes or credentials).
        message: String,
    },
    /// Runs `command` and replies with its outcome (typed refusal, never silent).
    Request {
        /// The command to run.
        command: Box<PipelineCommand>,
        /// Receives the outcome (bounded; a dropped receiver is ignored).
        reply: Sender<Result<(), CommandRejected>>,
    },
    /// Signals the pipeline thread to stop gracefully.
    Shutdown,
}

/// Input payload for an arriving LMU shared memory snapshot.
#[derive(Debug)]
pub struct PipelineLmuInput {
    /// The copied LMU shared memory snapshot.
    pub snapshot: Box<RawSnapshot>,
    /// Whether at least one snapshot was dropped due to reader queue saturation.
    pub overrun_before: bool,
    /// Measured reader throughput statistics.
    pub stats: Option<ReaderStats>,
}

/// Input message processed by [`DriverPipeline`].
#[derive(Debug)]
pub enum PipelineInput {
    /// Raw snapshot arrived from the reader thread.
    Lmu(Box<PipelineLmuInput>),
    /// Synthetic demo tick.
    #[cfg(feature = "demo")]
    Demo(Box<synthetic::SyntheticTick>),
    /// Discrete control or lifecycle command.
    Command(PipelineCommand),
    /// Periodic housekeeping tick (100 ms timeout = 10 Hz).
    Tick,
}

/// Execution summary returned when the pipeline terminates.
#[derive(Debug, Clone, Default)]
pub struct PipelineSummary {
    /// Total new physics frames processed and fanned out.
    pub frames_processed: u64,
    /// Total snapshots dropped due to reader queue overruns.
    pub reader_overruns: u64,
    /// Total pipeline discontinuities encountered (restarts, jumps, resets).
    pub discontinuities: u64,
}

/// Core driver-side pipeline coordinating acquisition, normalisation, calculation,
/// recording and streaming.
pub struct DriverPipeline {
    clock: AppClock,
    normalizer: Normalizer,
    engines: EngineHub,
    session_machine: DriverSessionMachine,
    recorder: Option<RecorderSupervisor>,
    net: Option<NetPublisher>,
    ui_hub: Arc<UiHub>,
    sink: Option<Arc<dyn UiSink>>,
    skew_tracker: SkewTracker,
    origin: Origin,
    status_slot: Option<StatusSlot>,
    streaming_flag: Option<Arc<AtomicBool>>,
    reader_diagnostics: Option<ReaderDiagnosticsSource>,
    last_reader_state: Option<ReaderState>,
    last_transport_state: Option<transport::ConnectionState>,
    last_net_status_mono_ns: Option<u64>,

    // Monotonic state tracking
    sequence: u64,
    last_frame: Option<TelemetryFrame>,
    last_scoring: Option<ScoringState>,
    active_identity: Option<SessionIdentity>,
    last_reader_stats: Option<ReaderStats>,
    last_keyframe_mono_ns: u64,
    last_telemetry_mono_ns: Option<u64>,
    last_frame_received_mono_ns: Option<u64>,
    last_scoring_change_mono_ns: Option<u64>,
    last_engine_state_mono_ns: u64,
    last_status_mono_ns: u64,
    in_realtime: bool,
    discontinuity_pending: bool,
    gap_before: bool,
    is_first_frame: bool,
    stalled_notified: bool,

    // Counters
    reader_overruns: u64,
    pipeline_overruns: u64,
    discontinuities: u64,
    frames_processed: u64,
    pipeline_latency_ns: u64,
}

impl DriverPipeline {
    /// Creates a new driver pipeline.
    pub fn new(
        clock: AppClock,
        session_settings: DriverSessionSettings,
        engine_config: EngineHubConfig,
        recorder: Option<RecorderSupervisor>,
        net: Option<NetPublisher>,
        ui_hub: Arc<UiHub>,
        sink: Option<Arc<dyn UiSink>>,
    ) -> Result<Self, PipelineError> {
        let origin = session_settings.origin;
        if origin == Origin::Synthetic && (recorder.is_some() || net.is_some()) {
            return Err(PipelineError::Channel(
                "synthetic pipeline must not have recorder or network sinks".into(),
            ));
        }

        let normalizer = Normalizer::new();
        let engines = EngineHub::with_config(engine_config)
            .map_err(|e| PipelineError::Channel(e.to_string()))?;
        let session_machine = DriverSessionMachine::new(session_settings);
        let skew_tracker = SkewTracker::new();

        Ok(Self {
            clock,
            normalizer,
            engines,
            session_machine,
            recorder,
            net,
            ui_hub,
            sink,
            skew_tracker,
            origin,
            status_slot: None,
            streaming_flag: None,
            reader_diagnostics: None,
            last_reader_state: None,
            last_transport_state: None,
            last_net_status_mono_ns: None,
            sequence: 0,
            last_frame: None,
            last_scoring: None,
            active_identity: None,
            last_reader_stats: None,
            last_keyframe_mono_ns: 0,
            last_telemetry_mono_ns: None,
            last_frame_received_mono_ns: None,
            last_scoring_change_mono_ns: None,
            last_engine_state_mono_ns: 0,
            last_status_mono_ns: 0,
            in_realtime: true,
            discontinuity_pending: true,
            gap_before: false,
            is_first_frame: true,
            stalled_notified: false,
            reader_overruns: 0,
            pipeline_overruns: 0,
            discontinuities: 0,
            frames_processed: 0,
            pipeline_latency_ns: 0,
        })
    }

    /// Publishes every composite status into `slot` (read by `AppCore::status`).
    pub fn set_status_slot(&mut self, slot: StatusSlot) {
        self.status_slot = Some(slot);
    }

    /// Mirrors the session machine's "streaming" state into `flag` as soon as it changes.
    pub fn set_streaming_flag(&mut self, flag: Arc<AtomicBool>) {
        flag.store(self.session_machine.is_streaming(), Ordering::Relaxed);
        self.streaming_flag = Some(flag);
    }

    fn sync_streaming_flag(&self) {
        if let Some(ref flag) = self.streaming_flag {
            flag.store(self.session_machine.is_streaming(), Ordering::Relaxed);
        }
    }

    /// Feeds the reader thread's diagnostics into the session machine (LMU not running,
    /// connected, incompatible) on the 10 Hz housekeeping tick.
    pub fn set_reader_diagnostics(&mut self, source: ReaderDiagnosticsSource) {
        self.reader_diagnostics = Some(source);
    }

    /// Whether this pipeline can stream (a relay link with session control is attached).
    pub fn relay_configured(&self) -> bool {
        self.net.as_ref().is_some_and(|n| n.control().is_some())
    }

    /// Returns the active origin of this pipeline run.
    pub fn origin(&self) -> Origin {
        self.origin
    }

    /// Returns the cumulative number of physics frames processed.
    pub fn frames_processed(&self) -> u64 {
        self.frames_processed
    }

    /// Returns the most recently computed capture-to-publish pipeline latency in nanoseconds.
    pub fn pipeline_latency_ns(&self) -> u64 {
        self.pipeline_latency_ns
    }

    /// Returns a reference to the inner session machine.
    pub fn session_machine(&self) -> &DriverSessionMachine {
        &self.session_machine
    }

    /// Returns a mutable reference to the inner recorder supervisor, if configured.
    pub fn recorder_mut(&mut self) -> Option<&mut RecorderSupervisor> {
        self.recorder.as_mut()
    }

    /// Returns a reference to the inner recorder supervisor, if configured.
    pub fn recorder(&self) -> Option<&RecorderSupervisor> {
        self.recorder.as_ref()
    }

    /// Returns a reference to the inner normalizer.
    pub fn normalizer(&self) -> &Normalizer {
        &self.normalizer
    }

    /// Returns a mutable reference to the inner engine hub.
    pub fn engines_mut(&mut self) -> &mut EngineHub {
        &mut self.engines
    }

    /// Processes a single discrete input step through the pipeline.
    pub fn step(&mut self, input: PipelineInput) -> Result<(), PipelineError> {
        match input {
            PipelineInput::Lmu(lmu_in) => {
                self.process_lmu_snapshot(&lmu_in.snapshot, lmu_in.overrun_before, lmu_in.stats)
            }
            #[cfg(feature = "demo")]
            PipelineInput::Demo(demo_tick) => self.process_demo_tick(*demo_tick),
            PipelineInput::Command(cmd) => self.process_command(cmd),
            PipelineInput::Tick => self.process_housekeeping_tick(),
        }
    }

    /// Processes one raw snapshot arriving from the LMU reader thread.
    fn process_lmu_snapshot(
        &mut self,
        snapshot: &RawSnapshot,
        overrun_before: bool,
        stats: Option<ReaderStats>,
    ) -> Result<(), PipelineError> {
        // Update normalizer's reader stats rate estimator if available:
        if let Some(ref st) = stats {
            self.normalizer.update_rates_from_stats(st);
            self.last_reader_stats = Some(st.clone());
        }

        // Check if reader queue saturation occurred before this snapshot:
        if overrun_before {
            self.reader_overruns = self.reader_overruns.saturating_add(1);
            self.discontinuities = self.discontinuities.saturating_add(1);
            self.discontinuity_pending = true;
            self.gap_before = true;
        }

        // Run LMU normalization:
        let output = self.normalizer.process(snapshot);

        // Capabilities and diagnostics:
        if (output.capabilities_changed || output.vehicle_capabilities.is_some())
            && let Some(ref sink) = self.sink
            && let Ok(v) = serde_json::to_value(self.normalizer.capability_map())
        {
            sink.emit(UiEvent::Capabilities(v));
        }

        // Diagnostics notices:
        for notice in &output.notices {
            if let Some(ref sink) = self.sink {
                sink.emit(UiEvent::Error(ErrorDto {
                    category: format!("{:?}", notice.code),
                    message: notice.message.clone(),
                    fatal: false,
                }));
            }
        }

        // Scoring updates:
        if let Some(ref scoring_state) = output.scoring {
            self.last_scoring = Some(scoring_state.clone());
            self.last_scoring_change_mono_ns = Some(snapshot.capture_mono_ns);

            if self.session_machine.is_streaming()
                && let Some(ref mut net) = self.net
            {
                net.publish_scoring(scoring_state, snapshot.capture_mono_ns);
            }
            if let Some(ref mut rec) = self.recorder
                && let Ok(bytes) = postcard::to_stdvec(scoring_state)
                && let Err(err) = rec.append_scoring(snapshot.capture_mono_ns, &bytes)
            {
                warn!(component = COMPONENT, error = %err, "failed to append scoring record to recorder");
            }
            if let Some(ref sink) = self.sink
                && let Ok(v) = serde_json::to_value(scoring_state)
            {
                sink.emit(UiEvent::Scoring(v));
            }
        }

        // Identity updates (the relay session is kept on a track/car change, Q3; only the data
        // for the next new relay session is refreshed):
        if let Some(ref identity) = output.identity {
            let changed = self.active_identity.as_ref() != Some(identity);
            self.active_identity = Some(identity.clone());
            if changed
                && let Some(control) = self.net.as_ref().and_then(|n| n.control().cloned())
                && let Err(err) = control.update_session(self.session_create())
            {
                warn!(component = COMPONENT, error = %err, "relay session data not updated");
            }
        }

        // Telemetry frame processing:
        if output.disposition == TelemetryDisposition::NewSampleAfterRegression {
            self.discontinuities = self.discontinuities.saturating_add(1);
            self.discontinuity_pending = true;
            self.gap_before = true;
        }

        if let Some(mut frame) = output.frame {
            let now_mono_ns = self.clock.now_ns();
            let now_unix_s = self.clock.now_unix_s();
            self.pipeline_latency_ns = now_mono_ns.saturating_sub(frame.capture_mono_ns);
            self.last_telemetry_mono_ns = Some(frame.capture_mono_ns);
            self.last_frame_received_mono_ns = Some(now_mono_ns);
            self.stalled_notified = false;

            self.in_realtime = frame.f64(ch::IN_REALTIME).is_none_or(|v| v > 0.5);

            let identity = self.active_identity.clone().unwrap_or_default();
            let actions = self.session_machine.step(SessionInputEvent::FrameReceived {
                capture_mono_ns: frame.capture_mono_ns,
                in_realtime: self.in_realtime,
                et_s: frame.source_et_s.unwrap_or(0.0),
                identity,
                session_id: None,
                now_unix_s: Some(now_unix_s),
            });
            self.execute_session_actions(actions);

            // Step engines suite:
            let engine_outcome = self
                .engines
                .process_frame(&mut frame, self.last_scoring.as_ref());

            // Route engine session events:
            for event in &engine_outcome.events {
                self.route_session_event(event, frame.capture_mono_ns, frame.source_et_s);
            }

            // Route stint completions to recorder index:
            for stint_ev in &engine_outcome.stint_events {
                if let race_engines::stint::StintEvent::Ended { number, .. } = stint_ev
                    && let Some(ref mut rec) = self.recorder
                {
                    let rec_stint = recorder::StintRecord {
                        stint_number: *number,
                        start_capture_ns: frame.capture_mono_ns,
                        end_capture_ns: Some(frame.capture_mono_ns),
                        start_lap: None,
                        end_lap: None,
                        driver: None,
                        metrics_json: None,
                    };
                    if let Err(e) = rec.record_stint(rec_stint) {
                        warn!(component = COMPONENT, error = %e, "failed to record stint in session index");
                    }
                }
            }

            // Route alerts to UI:
            for alert in &engine_outcome.alert_notices {
                self.route_alert_notice(alert);
            }

            // Throttled engine state emission (<= 2 Hz / 500 ms):
            if now_mono_ns.saturating_sub(self.last_engine_state_mono_ns) >= 500_000_000 {
                self.last_engine_state_mono_ns = now_mono_ns;
                if let Some(ref sink) = self.sink
                    && let Ok(v) = serde_json::to_value(engine_outcome.to_engine_state_dto())
                {
                    sink.emit(UiEvent::EngineState(v));
                }
            }

            // Step app channels:
            let latency_s = (self.pipeline_latency_ns as f32) / 1e9;
            let telem_age_s = (now_mono_ns.saturating_sub(frame.capture_mono_ns) as f32) / 1e9;
            let scoring_age_s = self
                .last_scoring_change_mono_ns
                .map_or(0.0, |t| (now_mono_ns.saturating_sub(t) as f32) / 1e9);

            let _ = frame.set_valid(ch::PIPELINE_LATENCY, Value::F32(latency_s));
            let _ = frame.set_valid(ch::SRC_TELEMETRY_AGE, Value::F32(telem_age_s));
            let _ = frame.set_valid(ch::SRC_SCORING_AGE, Value::F32(scoring_age_s));
            let _ = frame.set_valid(ch::NORMALIZER_FORMULA_VERSION, Value::U32(1));

            let norm_stats = self.normalizer.stats();
            let _ = frame.set_valid(
                ch::SRC_ET_REGRESSIONS,
                Value::U32(norm_stats.et_regressions as u32),
            );
            let _ = frame.set_valid(
                ch::SRC_DUPLICATE_FRAMES,
                Value::U64(norm_stats.duplicate_frames),
            );
            let _ = frame.set_valid(
                ch::SRC_INVALID_SAMPLES,
                Value::U64(norm_stats.invalid_samples),
            );

            if let Some(ref rec) = self.recorder {
                let rec_dto = rec.status_dto();
                let _ = frame.set_valid(ch::REC_BYTES, Value::U64(rec_dto.bytes_written));
                let _ = frame.set_valid(ch::REC_FRAMES_DROPPED, Value::U64(rec_dto.frames_lost));
                if let Some(free) = rec_dto.disk_free_bytes {
                    let _ = frame.set_valid(ch::REC_DISK_FREE, Value::U64(free));
                }
            }

            // Canonical TLV block encoding (shared between Recorder and UiHub):
            let is_keyframe = self.is_first_frame
                || self.discontinuity_pending
                || (frame
                    .capture_mono_ns
                    .saturating_sub(self.last_keyframe_mono_ns)
                    >= 1_000_000_000);

            let mut canonical_buf = Vec::new();
            if is_keyframe {
                frame.encode_snapshot(&mut canonical_buf, RELAY_NS_UNKNOWN)?;
                self.last_keyframe_mono_ns = frame.capture_mono_ns;
                self.discontinuity_pending = false;
            } else if let Some(ref prev) = self.last_frame {
                frame.encode_delta(prev, &mut canonical_buf, RELAY_NS_UNKNOWN)?;
            } else {
                frame.encode_snapshot(&mut canonical_buf, RELAY_NS_UNKNOWN)?;
                self.last_keyframe_mono_ns = frame.capture_mono_ns;
            }

            let block: Arc<[u8]> = Arc::from(canonical_buf.into_boxed_slice());

            // FanOut (non-blocking):
            if let Some(ref mut rec) = self.recorder
                && let Err(err) = rec.append_telemetry(frame.capture_mono_ns, &block)
            {
                warn!(component = COMPONENT, error = %err, "failed to append telemetry record to recorder");
            }
            self.ui_hub
                .push_block_with_gap(Arc::clone(&block), is_keyframe, self.gap_before);
            self.gap_before = false;

            if self.session_machine.is_streaming()
                && let Some(ref mut net) = self.net
            {
                net.publish_frame(&frame, frame.capture_mono_ns);
                net.publish_weather(&frame, frame.capture_mono_ns);
            }

            self.last_frame = Some(frame);
            self.frames_processed = self.frames_processed.saturating_add(1);
            self.is_first_frame = false;
        }

        Ok(())
    }

    /// SESSION_CREATE data for the relay from the current LMU identity (unknown fields `None`).
    fn session_create(&self) -> SessionCreate {
        SessionCreate {
            identity: self.active_identity.clone().unwrap_or_default(),
            lmu_version: self.session_machine.game_version().map(|v| v.to_string()),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            schema_version: TELEMETRY_SCHEMA_VERSION,
            capabilities: self.normalizer.vehicle_capabilities(),
        }
    }

    /// Current SCHEMA_NEGOTIATE for the network stream.
    fn schema_negotiate(&self) -> SchemaNegotiate {
        SchemaNegotiate {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            capabilities: self.normalizer.capability_map().clone(),
            vehicle_capabilities: self.normalizer.vehicle_capabilities(),
        }
    }

    /// Forwards reader state changes (LMU not running / connected / incompatible) to the
    /// machine. `Stalled` is detected by the pipeline's own frame timeout.
    fn poll_reader_state(&mut self) {
        let Some(diag) = self
            .reader_diagnostics
            .as_ref()
            .and_then(|src| src.load().as_ref().clone())
        else {
            return;
        };
        if self.last_reader_state == Some(diag.state) {
            return;
        }
        self.last_reader_state = Some(diag.state);
        if diag.state == ReaderState::Stalled {
            return;
        }
        let detail = diag.last_error.clone().or_else(|| diag.note.clone());
        let actions = self
            .session_machine
            .step(SessionInputEvent::ReaderStateChanged {
                state: diag.state,
                detail,
                game_version: None,
            });
        self.execute_session_actions(actions);
    }

    /// Feeds transport connection state changes to the machine (FAILED stops streaming).
    fn poll_transport_state(&mut self) {
        let Some(state) = self
            .net
            .as_ref()
            .and_then(NetPublisher::link_snapshot)
            .map(|s| s.status.state)
        else {
            return;
        };
        if self.last_transport_state == Some(state) {
            return;
        }
        self.last_transport_state = Some(state);
        let actions = self
            .session_machine
            .step(SessionInputEvent::TransportStateChanged(state));
        self.execute_session_actions(actions);
    }

    /// DRIVER_STATUS and RECORDING_STATUS for the engineers (1 Hz while streaming).
    fn publish_net_status(&mut self, now_mono_ns: u64) {
        if !self.session_machine.is_streaming() {
            return;
        }
        if self
            .last_net_status_mono_ns
            .is_some_and(|last| now_mono_ns.saturating_sub(last) < NET_STATUS_INTERVAL_NS)
        {
            return;
        }
        self.last_net_status_mono_ns = Some(now_mono_ns);
        let link = self.net.as_ref().and_then(NetPublisher::link_snapshot);
        let driver_status = self.driver_status(link.as_ref());
        let recording_status = self.wire_recording_status();
        if let Some(ref mut net) = self.net {
            net.publish_status(driver_status, recording_status, now_mono_ns);
        }
    }

    /// Layered driver health for DRIVER_STATUS (unknown is reported as degraded, never ok).
    fn driver_status(&self, link: Option<&LinkSnapshot>) -> DriverStatus {
        let health = |level: HealthLevel, detail: Option<String>| ComponentHealth { level, detail };
        let lmu_state = self.session_machine.lmu_source_state();
        let lmu_source = match lmu_state {
            LmuSourceState::Live => health(HealthLevel::Ok, None),
            LmuSourceState::Connecting | LmuSourceState::Stale => {
                health(HealthLevel::Degraded, None)
            }
            LmuSourceState::NotRunning | LmuSourceState::Paused => {
                health(HealthLevel::Inactive, None)
            }
            LmuSourceState::Error => health(
                HealthLevel::Failed,
                self.session_machine.lmu_source_status().detail,
            ),
        };
        let processing = if self.frames_processed == 0 {
            health(HealthLevel::Inactive, None)
        } else if self.reader_overruns > 0 {
            health(
                HealthLevel::Degraded,
                Some(format!("{} reader overruns", self.reader_overruns)),
            )
        } else {
            health(HealthLevel::Ok, None)
        };
        let recording = match self.recorder.as_ref().map(RecorderSupervisor::status) {
            None => health(HealthLevel::Inactive, Some("recording disabled".to_owned())),
            Some(RecordingStatus::Active) => health(HealthLevel::Ok, None),
            Some(RecordingStatus::Off) => health(HealthLevel::Inactive, None),
            Some(RecordingStatus::Suspended) => {
                health(HealthLevel::Degraded, Some("disk space low".to_owned()))
            }
            Some(RecordingStatus::Failed) => health(HealthLevel::Failed, None),
        };
        let network = match link.map(|l| l.status.state) {
            Some(transport::ConnectionState::Connected) => health(HealthLevel::Ok, None),
            Some(s) if s.is_session_up() => health(HealthLevel::Degraded, None),
            Some(_) => health(HealthLevel::Degraded, Some("reconnecting".to_owned())),
            None => health(HealthLevel::Degraded, Some("link state unknown".to_owned())),
        };
        let stats = self.last_reader_stats.as_ref();
        DriverStatus {
            lmu_state,
            lmu_source,
            processing,
            recording,
            network,
            rates: DriverRates {
                telemetry_read_hz: stats
                    .and_then(|s| s.telemetry_rate.rate_hz())
                    .map(|r| r as f32),
                scoring_read_hz: stats
                    .and_then(|s| s.scoring_rate.rate_hz())
                    .map(|r| r as f32),
                frames_sent_hz: link
                    .and_then(|l| l.metrics.frames_out_per_s)
                    .map(|r| r as f32),
                bytes_sent_per_s: link
                    .and_then(|l| l.metrics.bytes_out_per_s)
                    .map(|r| r as u64),
                frames_dropped_total: link.map_or(0, |l| l.metrics.dropped_obsolete),
            },
            clock: link.and_then(|l| l.clock).map(|c| ClockSummary {
                offset_ns: c.offset_ns,
                uncertainty_ns: c.uncertainty_ns,
            }),
        }
    }

    /// RECORDING_STATUS for the engineers.
    fn wire_recording_status(&self) -> WireRecordingStatus {
        let dto = self.recording_status_dto();
        let state = match self.recorder.as_ref().map(RecorderSupervisor::status) {
            Some(RecordingStatus::Active) => RecordingState::Active,
            Some(RecordingStatus::Failed | RecordingStatus::Suspended) => RecordingState::Failed,
            Some(RecordingStatus::Off) | None => RecordingState::Off,
        };
        WireRecordingStatus {
            state,
            recording_id: dto.recording_id,
            bytes_written: dto.bytes_written,
            frames_written: dto.frames_written,
            frames_lost: dto.frames_lost,
            disk_free_bytes: dto.disk_free_bytes,
            error: dto.error,
        }
    }

    /// Recording block of `core:status` (`disabled` when the pipeline has no recorder).
    fn recording_status_dto(&self) -> RecordingStatusDto {
        self.recorder
            .as_ref()
            .map(RecorderSupervisor::status_dto)
            .unwrap_or_else(|| RecordingStatusDto {
                state: "disabled".into(),
                recording_id: None,
                bytes_written: 0,
                frames_written: 0,
                frames_lost: 0,
                disk_free_bytes: None,
                error: None,
            })
    }

    /// Processes the 100 ms timeout housekeeping tick (10 Hz).
    fn process_housekeeping_tick(&mut self) -> Result<(), PipelineError> {
        let now_mono_ns = self.clock.now_ns();

        // 0. Reader and transport state changes, reliable queue drain, status to engineers:
        self.poll_reader_state();
        self.poll_transport_state();
        if self.session_machine.is_streaming()
            && let Some(ref mut net) = self.net
        {
            net.drain_reliable();
        }
        self.publish_net_status(now_mono_ns);

        // 1. Source age check & stall detection:
        if let Some(last_recv) = self.last_frame_received_mono_ns {
            let elapsed_since_frame = now_mono_ns.saturating_sub(last_recv);

            if elapsed_since_frame >= 100_000_000 {
                // Emit small SRC-age block with NaN ET so ages keep advancing on UI without polluting rings:
                self.emit_age_block(now_mono_ns);
            }

            if elapsed_since_frame >= 1_000_000_000 && !self.stalled_notified {
                self.stalled_notified = true;
                let actions = self.session_machine.step(SessionInputEvent::FrameTimeout {
                    in_realtime: self.in_realtime,
                });
                self.execute_session_actions(actions);
            }
        }

        // 2. Idle alerts evaluation:
        if let Some(ref last_frame) = self.last_frame {
            let alert_clock = AlertClock::from_mono_ns(now_mono_ns, None);
            let notices = self.engines.evaluate_idle_alerts(alert_clock, last_frame);
            for alert in &notices {
                self.route_alert_notice(alert);
            }
        }

        // 3. Recorder events drain:
        if let Some(ref mut rec) = self.recorder {
            let outcome = rec.poll_events();
            let source_et_s = self.last_frame.as_ref().and_then(|f| f.source_et_s);
            for evt in outcome.session_events {
                self.route_session_event(&evt, now_mono_ns, source_et_s);
            }
        }

        // 4. Clock skew tracking (sleep/resume):
        if let SkewStatus::Discontinuity { skew, .. } = self.skew_tracker.check() {
            self.discontinuities = self.discontinuities.saturating_add(1);
            let actions = self
                .session_machine
                .step(SessionInputEvent::ClockSkewDetected {
                    skew_s: skew.as_secs_f64(),
                });
            self.execute_session_actions(actions);
            self.discontinuity_pending = true;
            self.gap_before = true;
        }

        // 5. Periodic status broadcast (4 Hz / 250 ms):
        if now_mono_ns.saturating_sub(self.last_status_mono_ns) >= 250_000_000 {
            self.last_status_mono_ns = now_mono_ns;
            self.emit_status(now_mono_ns);
        }

        Ok(())
    }

    /// Emits a small aging block with `SRC_TELEMETRY_AGE` and `SRC_SCORING_AGE` with NaN ET.
    fn emit_age_block(&mut self, now_mono_ns: u64) {
        let telem_age_s = self
            .last_frame_received_mono_ns
            .map_or(0.0, |t| (now_mono_ns.saturating_sub(t) as f32) / 1e9);
        let scoring_age_s = self
            .last_scoring_change_mono_ns
            .map_or(0.0, |t| (now_mono_ns.saturating_sub(t) as f32) / 1e9);

        let header = TlvHeader {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            capture_mono_ns: now_mono_ns,
            capture_relay_ns_est: RELAY_NS_UNKNOWN,
            source_et_s: f64::NAN,
        };

        let mut buf = Vec::new();
        let mut enc = TlvEncoder::begin(&mut buf, &header);
        let _ = enc.push(
            ch::SRC_TELEMETRY_AGE,
            &Sample::valid(Value::F32(telem_age_s)),
        );
        let _ = enc.push(
            ch::SRC_SCORING_AGE,
            &Sample::valid(Value::F32(scoring_age_s)),
        );
        let _ = enc.finish();

        let block: Arc<[u8]> = Arc::from(buf.into_boxed_slice());
        if let Some(ref mut rec) = self.recorder {
            let _ = rec.append_telemetry(now_mono_ns, &block);
        }
        self.ui_hub.push_block(Arc::clone(&block), false);
    }

    /// Emits a full stale block setting Physics, Scoring and Environment channels to `Quality::Stale`.
    fn emit_stale_block(&mut self) {
        let now_mono_ns = self.clock.now_ns();
        self.sequence = self.sequence.wrapping_add(1);
        let mut stale_frame = TelemetryFrame::new(self.sequence, now_mono_ns, None);

        for def in registry::CHANNELS.iter() {
            if matches!(
                def.frequency,
                FrequencyClass::Physics | FrequencyClass::Scoring | FrequencyClass::Environment
            ) {
                let _ = stale_frame.set(def.id, Sample::stale());
            }
        }

        let mut buf = Vec::new();
        if stale_frame
            .encode_snapshot(&mut buf, RELAY_NS_UNKNOWN)
            .is_ok()
        {
            let block: Arc<[u8]> = Arc::from(buf.into_boxed_slice());
            self.ui_hub.push_block(Arc::clone(&block), true);
        }
    }

    /// Routes an alert notice to the UI sink.
    fn route_alert_notice(&self, alert: &AlertNotice) {
        if let Some(ref sink) = self.sink {
            sink.emit(UiEvent::Alert(AlertDto {
                id: alert.alert.id.to_string(),
                name: alert.alert.rule_id.clone(),
                level: match alert.alert.severity {
                    Severity::Info | Severity::Notice => AlertLevel::Info,
                    Severity::Warning => AlertLevel::Warning,
                    Severity::Critical => AlertLevel::Critical,
                },
                raised: matches!(alert.state, AlertState::Raised | AlertState::Active),
                message: alert.alert.message.clone(),
                timestamp_ns: (alert.alert.raised_s * 1e9) as u64,
            }));
        }
    }

    /// Routes a session event to recorder, net publisher, and UI sink.
    fn route_session_event(
        &mut self,
        event: &SessionEvent,
        mono_ns: u64,
        source_et_s: Option<f64>,
    ) {
        if let Some(ref mut rec) = self.recorder {
            match postcard::to_stdvec(event) {
                Ok(bytes) => {
                    if let Err(err) = rec.append_session_event(mono_ns, &bytes) {
                        warn!(component = COMPONENT, error = %err, "failed to append session event to recorder");
                    }
                }
                Err(err) => {
                    warn!(component = COMPONENT, error = %err, "failed to encode session event with postcard");
                }
            }
        }

        // If this is a completed lap, also index it in the recorder:
        if let SessionEvent::LapCompleted { lap, time_s, valid } = event {
            let stint_no = self.engines.stint.snapshot().number.value;
            let lap_rec = LapRecord {
                lap_number: *lap,
                stint_number: stint_no,
                start_capture_ns: mono_ns,
                end_capture_ns: Some(mono_ns),
                lap_time_s: *time_s,
                sector_times_s: [None, None, None],
                valid: Some(*valid),
                pit_lap: None,
                metrics_json: None,
            };
            if let Some(ref mut rec) = self.recorder
                && let Err(err) = rec.record_lap(lap_rec)
            {
                warn!(component = COMPONENT, error = %err, "failed to record lap in session index");
            }
        }

        if self.session_machine.is_streaming()
            && let Some(ref mut net) = self.net
        {
            let at = EventTime {
                source_et_s,
                mono_ns,
            };
            let record = EventRecord::new(at, None, event.clone());
            net.push_session_event(record, mono_ns);
        }

        if let Some(ref sink) = self.sink
            && let Ok(v) = serde_json::to_value(event)
        {
            sink.emit(UiEvent::SessionEvent(v));
        }
    }

    /// Builds the composite status of this pipeline (no hard-coded values: unknown is unknown).
    pub fn build_status(&self) -> CoreStatus {
        let link = self.net.as_ref().and_then(NetPublisher::link_snapshot);
        let network_status = network_status_dto(
            link.as_ref(),
            self.relay_configured(),
            self.session_machine.is_streaming(),
        );

        let telem_rate = self
            .last_reader_stats
            .as_ref()
            .and_then(|s| s.telemetry_rate.rate_hz())
            .map(|r| r as f32);
        let scoring_rate = self
            .last_reader_stats
            .as_ref()
            .and_then(|s| s.scoring_rate.rate_hz())
            .map(|r| r as f32);
        let net_events_dropped = self
            .net
            .as_ref()
            .map_or(0, |n| n.stats().reliable_events_dropped);
        let recording_status = self.recording_status_dto();
        let recorder_overrun = recording_status.frames_lost;
        let net_dropped_obsolete = link.as_ref().map_or(0, |l| l.metrics.dropped_obsolete);

        CoreStatus {
            mode: AppMode::Driver,
            origin: self.origin,
            lmu: self.session_machine.lmu_source_status(),
            session: self.session_machine.session_status_dto(),
            recording: recording_status,
            network: network_status,
            rates: RatesDto {
                telemetry_read_hz: telem_rate,
                scoring_read_hz: scoring_rate,
                frames_sent_hz: link
                    .as_ref()
                    .and_then(|l| l.metrics.frames_out_per_s)
                    .map(|r| r as f32),
                bytes_sent_per_s: link
                    .as_ref()
                    .and_then(|l| l.metrics.bytes_out_per_s)
                    .map(|r| r as u64),
                frames_dropped_total: net_dropped_obsolete,
            },
            drops: DropsDto {
                reader_overrun: self.reader_overruns,
                pipeline_overrun: self.pipeline_overruns,
                recorder_overrun,
                net_dropped_obsolete,
                net_events_dropped,
                ui_dropped: self.ui_hub.total_ui_dropped(),
            },
            engineer: None,
        }
    }

    /// Emits the composite `core:status` JSON event and stores it for `AppCore::status`.
    fn emit_status(&self, _now_mono_ns: u64) {
        let status = self.build_status();
        if let Some(ref slot) = self.status_slot {
            *slot.lock() = Some(status.clone());
        }
        if let Some(ref sink) = self.sink {
            sink.emit(UiEvent::Status(Box::new(status)));
        }
    }

    /// Reports a refused fire-and-forget command or relay control failure (never silent).
    fn report_rejection(&self, rejection: &CommandRejected) {
        warn!(component = COMPONENT, reason = %rejection, "pipeline command refused");
        if let Some(ref sink) = self.sink {
            sink.emit(UiEvent::Error(ErrorDto {
                category: "command".to_owned(),
                message: rejection.to_string(),
                fatal: false,
            }));
        }
    }

    /// Processes an external pipeline command. A refused command returns
    /// [`PipelineError::Rejected`] with the typed reason.
    fn process_command(&mut self, cmd: PipelineCommand) -> Result<(), PipelineError> {
        match cmd {
            PipelineCommand::Request { command, reply } => match self.process_command(*command) {
                Ok(()) => {
                    let _ = reply.try_send(Ok(()));
                    Ok(())
                }
                Err(PipelineError::Rejected(rejected)) => {
                    let _ = reply.try_send(Err(rejected));
                    Ok(())
                }
                Err(other) => Err(other),
            },
            PipelineCommand::SnapshotNeeded => {
                if let Some(ref mut net) = self.net {
                    net.request_snapshot();
                }
                self.discontinuity_pending = true;
                Ok(())
            }
            PipelineCommand::StartStreaming => {
                if self.origin == Origin::Synthetic {
                    return Err(CommandRejected::SyntheticNeverStreamed.into());
                }
                if !self.relay_configured() {
                    return Err(CommandRejected::NetworkUnavailable.into());
                }
                if !self.session_machine.is_logged_in() {
                    return Err(CommandRejected::NotLoggedIn.into());
                }
                let actions = self
                    .session_machine
                    .step(SessionInputEvent::ManualStartStreaming);
                Ok(self.execute_session_actions_checked(actions)?)
            }
            PipelineCommand::StopStreaming => {
                if !self.relay_configured() {
                    return Err(CommandRejected::NetworkUnavailable.into());
                }
                let actions = self
                    .session_machine
                    .step(SessionInputEvent::ManualStopStreaming);
                Ok(self.execute_session_actions_checked(actions)?)
            }
            PipelineCommand::EndSession => {
                let actions = self
                    .session_machine
                    .step(SessionInputEvent::ManualEndSession);
                let ends_relay = actions.contains(&SessionAction::EndStreamingSession);
                let result = self.execute_session_actions_checked(actions);
                if let Some(ref mut rec) = self.recorder {
                    let events = rec.on_session_end();
                    let now_mono_ns = self.clock.now_ns();
                    let source_et_s = self.last_frame.as_ref().and_then(|f| f.source_et_s);
                    for evt in events {
                        self.route_session_event(&evt, now_mono_ns, source_et_s);
                    }
                }
                // A relay session kept for a resume (streaming stopped) is ended as well.
                if !ends_relay
                    && let Some(control) = self.net.as_ref().and_then(|n| n.control().cloned())
                {
                    control.end_session().map_err(CommandRejected::Control)?;
                }
                Ok(result?)
            }
            PipelineCommand::SetRecording(enabled) => {
                let rec = self
                    .recorder
                    .as_mut()
                    .ok_or(CommandRejected::RecorderUnavailable)?;
                let events = rec
                    .set_enabled(enabled)
                    .map_err(|e| CommandRejected::Recorder(e.to_string()))?;
                let now_mono_ns = self.clock.now_ns();
                let source_et_s = self.last_frame.as_ref().and_then(|f| f.source_et_s);
                for evt in events {
                    self.route_session_event(&evt, now_mono_ns, source_et_s);
                }
                Ok(())
            }
            PipelineCommand::AddMarker(text) => {
                let now = self.clock.now_ns();
                let rec = self
                    .recorder
                    .as_ref()
                    .ok_or(CommandRejected::RecorderUnavailable)?;
                if rec.active_session_id().is_none() {
                    return Err(CommandRejected::NotRecording.into());
                }
                rec.append_marker(now, &text)
                    .map_err(|e| CommandRejected::Recorder(e.to_string()))?;
                Ok(())
            }
            PipelineCommand::UpdateSettings(settings) => {
                let actions = self
                    .session_machine
                    .step(SessionInputEvent::SettingsUpdated(settings));
                self.execute_session_actions(actions);
                Ok(())
            }
            PipelineCommand::AckAlert(alert_id) => {
                self.engines
                    .alert
                    .acknowledge(alert_id)
                    .map_err(|_| CommandRejected::UnknownAlert(alert_id))?;
                Ok(())
            }
            PipelineCommand::AuthStatus { logged_in } => {
                let relay_configured = self.relay_configured();
                let actions = self
                    .session_machine
                    .step(SessionInputEvent::AuthStatusChanged {
                        logged_in,
                        relay_configured,
                    });
                self.execute_session_actions(actions);
                Ok(())
            }
            PipelineCommand::NetSessionReady => {
                let schema = self.schema_negotiate();
                if let Some(ref mut net) = self.net {
                    net.reset_stream();
                    net.set_schema(schema);
                }
                self.discontinuity_pending = true;
                Ok(())
            }
            PipelineCommand::NetNotice { category, message } => {
                if let Some(ref sink) = self.sink {
                    sink.emit(UiEvent::Error(ErrorDto {
                        category,
                        message,
                        fatal: false,
                    }));
                }
                Ok(())
            }
            PipelineCommand::Shutdown => {
                if let Some(ref mut rec) = self.recorder {
                    let events = rec.on_session_end();
                    let now_mono_ns = self.clock.now_ns();
                    let source_et_s = self.last_frame.as_ref().and_then(|f| f.source_et_s);
                    for evt in events {
                        self.route_session_event(&evt, now_mono_ns, source_et_s);
                    }
                }
                Ok(())
            }
        }
    }

    /// Executes session actions; returns the first relay control failure, if any.
    fn execute_session_actions_checked(
        &mut self,
        actions: Vec<SessionAction>,
    ) -> Result<(), CommandRejected> {
        let mut failure = None;
        for action in actions {
            if let Err(err) = self.execute_session_action(action)
                && failure.is_none()
            {
                failure = Some(err);
            }
        }
        self.sync_streaming_flag();
        failure.map_or(Ok(()), Err)
    }

    /// Executes discrete actions returned by the [`DriverSessionMachine`]; relay control
    /// failures are reported to the UI (never silent).
    fn execute_session_actions(&mut self, actions: Vec<SessionAction>) {
        if let Err(rejection) = self.execute_session_actions_checked(actions) {
            self.report_rejection(&rejection);
        }
    }

    /// Executes one session action.
    fn execute_session_action(&mut self, action: SessionAction) -> Result<(), CommandRejected> {
        {
            match action {
                SessionAction::StartRecording(meta) => {
                    if let Some(ref mut rec) = self.recorder {
                        match rec.on_session_ready(meta) {
                            Ok(events) => {
                                let now_mono_ns = self.clock.now_ns();
                                let source_et_s =
                                    self.last_frame.as_ref().and_then(|f| f.source_et_s);
                                for evt in events {
                                    self.route_session_event(&evt, now_mono_ns, source_et_s);
                                }
                            }
                            Err(err) => {
                                warn!(
                                    component = COMPONENT,
                                    error = %err,
                                    "failed to start local recording session"
                                );
                            }
                        }
                    }
                }
                SessionAction::FinishRecordingSegment | SessionAction::FinishRecordingSession => {
                    if let Some(ref mut rec) = self.recorder {
                        let events = rec.on_session_stalled();
                        let now_mono_ns = self.clock.now_ns();
                        let source_et_s = self.last_frame.as_ref().and_then(|f| f.source_et_s);
                        for evt in events {
                            self.route_session_event(&evt, now_mono_ns, source_et_s);
                        }
                    }
                }
                SessionAction::ResumeRecordingSegment => {
                    // Segment resumes automatically on next appended frame
                }
                SessionAction::StartStreaming => {
                    info!(component = COMPONENT, "live streaming starting");
                    let Some(control) = self.net.as_ref().and_then(|n| n.control().cloned()) else {
                        self.revert_streaming();
                        return Err(CommandRejected::NetworkUnavailable);
                    };
                    let session = self.session_create();
                    if let Some(ref mut net) = self.net {
                        net.reset_stream();
                    }
                    if let Err(err) = control.start(session) {
                        self.revert_streaming();
                        return Err(CommandRejected::Control(err));
                    }
                }
                SessionAction::StopStreaming => {
                    info!(
                        component = COMPONENT,
                        "live streaming stopped (session kept)"
                    );
                    if let Some(control) = self.net.as_ref().and_then(|n| n.control().cloned()) {
                        control.stop().map_err(CommandRejected::Control)?;
                    }
                }
                SessionAction::EndStreamingSession => {
                    info!(component = COMPONENT, "live streaming session ending");
                    if let Some(control) = self.net.as_ref().and_then(|n| n.control().cloned()) {
                        control.end_session().map_err(CommandRejected::Control)?;
                    }
                }
                SessionAction::SendStaleBlock => {
                    self.emit_stale_block();
                }
                SessionAction::SendSchemaNegotiate => {
                    let schema = self.schema_negotiate();
                    if let Some(ref mut net) = self.net {
                        net.set_schema(schema);
                    }
                }
                SessionAction::SendSnapshot => {
                    if let Some(ref mut net) = self.net {
                        net.request_snapshot();
                    }
                    self.discontinuity_pending = true;
                }
                SessionAction::EmitUiReset => {
                    self.discontinuity_pending = true;
                    self.gap_before = true;
                }
                SessionAction::EmitSessionEvent(event) => {
                    let mono_ns = self.clock.now_ns();
                    let source_et = self.last_frame.as_ref().and_then(|f| f.source_et_s);
                    self.route_session_event(&event, mono_ns, source_et);
                }
                SessionAction::RecordDiscontinuity(_reason) => {
                    self.discontinuities = self.discontinuities.saturating_add(1);
                    self.discontinuity_pending = true;
                    self.gap_before = true;
                }
            }
        }
        Ok(())
    }

    /// Returns the machine to "not streaming" after a start that could not be queued.
    fn revert_streaming(&mut self) {
        let _ = self
            .session_machine
            .step(SessionInputEvent::ManualStopStreaming);
        self.sync_streaming_flag();
    }

    /// Runs the pipeline event loop until `stop_flag` is signaled or receiver channels disconnect.
    pub fn run(
        mut self,
        snapshot_rx: Receiver<ReaderSnapshotMessage>,
        cmd_rx: Receiver<PipelineCommand>,
        stop_flag: Arc<AtomicBool>,
    ) -> PipelineSummary {
        info!(
            component = COMPONENT,
            "pipeline thread started (NORMAL priority, 100ms housekeeping tick)"
        );

        let mut next_tick = Instant::now() + HOUSEKEEPING_INTERVAL;
        while !stop_flag.load(Ordering::Relaxed) {
            crossbeam_channel::select! {
                recv(snapshot_rx) -> msg => {
                    match msg {
                        Ok(item) => {
                            if let Err(err) = self.step(PipelineInput::Lmu(Box::new(PipelineLmuInput {
                                snapshot: Box::new(item.snapshot),
                                overrun_before: item.overrun_before,
                                stats: item.stats,
                            }))) {
                                error!(component = COMPONENT, error = %err, "pipeline error processing snapshot");
                            }
                        }
                        Err(_) => {
                            debug!(component = COMPONENT, "reader snapshot channel closed; exiting loop");
                            break;
                        }
                    }
                }
                recv(cmd_rx) -> msg => {
                    if let Ok(cmd) = msg {
                        if matches!(cmd, PipelineCommand::Shutdown) {
                            // §10: drain what the (already stopped) reader queued, so every
                            // acquired snapshot is processed and recorded before finalizing.
                            self.drain_snapshots(&snapshot_rx);
                        }
                        if self.handle_command_input(cmd) {
                            break;
                        }
                    }
                }
                default(next_tick.saturating_duration_since(Instant::now())) => {}
            }
            // Housekeeping is due every 100 ms whether or not input keeps arriving (a 100 Hz
            // stream must not starve status, stall detection, alerts or the recorder drain).
            if Instant::now() >= next_tick {
                if let Err(err) = self.step(PipelineInput::Tick) {
                    error!(component = COMPONENT, error = %err, "pipeline housekeeping tick error");
                }
                next_tick = Instant::now() + HOUSEKEEPING_INTERVAL;
            }
        }

        info!(
            component = COMPONENT,
            "pipeline thread stopping; finalizing recording"
        );
        if let Some(ref mut rec) = self.recorder {
            let events = rec.on_session_end();
            let now_mono_ns = self.clock.now_ns();
            let source_et_s = self.last_frame.as_ref().and_then(|f| f.source_et_s);
            for evt in events {
                self.route_session_event(&evt, now_mono_ns, source_et_s);
            }
        }

        PipelineSummary {
            frames_processed: self.frames_processed,
            reader_overruns: self.reader_overruns,
            discontinuities: self.discontinuities,
        }
    }

    /// Processes every snapshot already queued by the reader (bounded by the queue capacity).
    fn drain_snapshots(&mut self, snapshot_rx: &Receiver<ReaderSnapshotMessage>) {
        for item in snapshot_rx.try_iter() {
            if let Err(err) = self.step(PipelineInput::Lmu(Box::new(PipelineLmuInput {
                snapshot: Box::new(item.snapshot),
                overrun_before: item.overrun_before,
                stats: item.stats,
            }))) {
                error!(component = COMPONENT, error = %err, "pipeline error processing snapshot");
            }
        }
    }

    /// Runs one command from the control queue; refusals of fire-and-forget commands are
    /// reported (`core:error`). Returns `true` for `Shutdown`.
    fn handle_command_input(&mut self, cmd: PipelineCommand) -> bool {
        let is_shutdown = matches!(cmd, PipelineCommand::Shutdown);
        match self.step(PipelineInput::Command(cmd)) {
            Ok(()) => {}
            Err(PipelineError::Rejected(rejection)) => self.report_rejection(&rejection),
            Err(err) => {
                error!(component = COMPONENT, error = %err, "pipeline error processing command");
            }
        }
        is_shutdown
    }

    /// Spawns the dedicated `"pipeline"` OS thread.
    pub fn spawn(
        self,
        snapshot_rx: Receiver<ReaderSnapshotMessage>,
        cmd_rx: Receiver<PipelineCommand>,
        stop_flag: Arc<AtomicBool>,
    ) -> Result<JoinHandle<PipelineSummary>, std::io::Error> {
        Builder::new()
            .name("pipeline".to_string())
            .spawn(move || self.run(snapshot_rx, cmd_rx, stop_flag))
    }

    /// Processes a single synthetic demo tick arriving from the synthetic provider.
    #[cfg(feature = "demo")]
    fn process_demo_tick(&mut self, tick: synthetic::SyntheticTick) -> Result<(), PipelineError> {
        let now_mono_ns = self.clock.now_ns();
        self.last_frame_received_mono_ns = Some(now_mono_ns);

        // Scoring updates:
        if let Some(ref scoring) = tick.scoring {
            self.last_scoring = Some(scoring.state.clone());
            self.last_scoring_change_mono_ns = Some(tick.capture_mono_ns);
            if let Some(ref sink) = self.sink
                && let Ok(v) = serde_json::to_value(&scoring.state)
            {
                sink.emit(UiEvent::Scoring(v));
            }
        }

        // Stale state / no frame:
        if tick.stale.is_some() || tick.frame.is_none() {
            if !self.stalled_notified {
                self.emit_stale_block();
                self.stalled_notified = true;
            }
            return Ok(());
        }

        let mut frame = match tick.frame {
            Some(f) => f,
            None => return Ok(()),
        };

        self.stalled_notified = false;
        self.last_telemetry_mono_ns = Some(frame.capture_mono_ns);
        self.pipeline_latency_ns = now_mono_ns.saturating_sub(frame.capture_mono_ns);

        // Process through EngineHub:
        let engine_out = self
            .engines
            .process_frame(&mut frame, self.last_scoring.as_ref());

        // Session events and alerts:
        for evt in &engine_out.events {
            self.route_session_event(evt, frame.capture_mono_ns, frame.source_et_s);
        }
        for alert in &engine_out.alert_notices {
            self.route_alert_notice(alert);
        }

        // App channels:
        let latency_s = (self.pipeline_latency_ns as f32) / 1e9;
        let scoring_age_s = self.last_scoring_change_mono_ns.map_or(0.0f32, |t| {
            (frame.capture_mono_ns.saturating_sub(t) as f32) / 1e9
        });

        if let Err(err) = frame.set_valid(ch::PIPELINE_LATENCY, Value::F32(latency_s)) {
            warn!(component = COMPONENT, error = %err, "failed to set demo PIPELINE_LATENCY");
        }
        if let Err(err) = frame.set_valid(ch::SRC_TELEMETRY_AGE, Value::F32(0.0)) {
            warn!(component = COMPONENT, error = %err, "failed to set demo SRC_TELEMETRY_AGE");
        }
        if let Err(err) = frame.set_valid(ch::SRC_SCORING_AGE, Value::F32(scoring_age_s)) {
            warn!(component = COMPONENT, error = %err, "failed to set demo SRC_SCORING_AGE");
        }
        if let Err(err) = frame.set_valid(ch::NORMALIZER_FORMULA_VERSION, Value::U32(1)) {
            warn!(component = COMPONENT, error = %err, "failed to set demo NORMALIZER_FORMULA_VERSION");
        }

        // Canonical TLV block encoding (shared between Recorder and UiHub):
        let is_keyframe = self.is_first_frame
            || self.discontinuity_pending
            || (frame
                .capture_mono_ns
                .saturating_sub(self.last_keyframe_mono_ns)
                >= 1_000_000_000);

        let mut canonical_buf = Vec::new();
        if is_keyframe {
            frame.encode_snapshot(&mut canonical_buf, RELAY_NS_UNKNOWN)?;
            self.last_keyframe_mono_ns = frame.capture_mono_ns;
            self.discontinuity_pending = false;
        } else if let Some(ref prev) = self.last_frame {
            frame.encode_delta(prev, &mut canonical_buf, RELAY_NS_UNKNOWN)?;
        } else {
            frame.encode_snapshot(&mut canonical_buf, RELAY_NS_UNKNOWN)?;
            self.last_keyframe_mono_ns = frame.capture_mono_ns;
        }

        let block: Arc<[u8]> = Arc::from(canonical_buf.into_boxed_slice());

        // FanOut (non-blocking):
        if let Some(ref mut rec) = self.recorder {
            let _ = rec.append_telemetry(frame.capture_mono_ns, &block);
        }
        self.ui_hub
            .push_block_with_gap(Arc::clone(&block), is_keyframe, self.gap_before);
        self.gap_before = false;

        if let Some(ref mut net) = self.net {
            net.publish_frame(&frame, frame.capture_mono_ns);
        }

        self.last_frame = Some(frame);
        self.frames_processed = self.frames_processed.saturating_add(1);
        self.is_first_frame = false;

        Ok(())
    }

    /// Runs the demo pipeline loop taking ticks from `demo_rx`.
    #[cfg(feature = "demo")]
    pub fn run_demo(
        mut self,
        demo_rx: Receiver<Box<synthetic::SyntheticTick>>,
        cmd_rx: Receiver<PipelineCommand>,
        stop_flag: Arc<AtomicBool>,
    ) -> PipelineSummary {
        info!(
            component = COMPONENT,
            "pipeline demo thread started (Origin::Synthetic, display-only)"
        );

        let mut next_tick = Instant::now() + HOUSEKEEPING_INTERVAL;
        while !stop_flag.load(Ordering::Relaxed) {
            crossbeam_channel::select! {
                recv(demo_rx) -> msg => {
                    match msg {
                        Ok(tick) => {
                            if let Err(err) = self.step(PipelineInput::Demo(tick)) {
                                error!(component = COMPONENT, error = %err, "pipeline error processing demo tick");
                            }
                        }
                        Err(_) => {
                            debug!(component = COMPONENT, "demo stream closed; exiting loop");
                            break;
                        }
                    }
                }
                recv(cmd_rx) -> msg => {
                    if let Ok(cmd) = msg
                        && self.handle_command_input(cmd)
                    {
                        break;
                    }
                }
                default(next_tick.saturating_duration_since(Instant::now())) => {}
            }
            // Housekeeping is due every 100 ms whether or not input keeps arriving (a 100 Hz
            // stream must not starve status, stall detection, alerts or the recorder drain).
            if Instant::now() >= next_tick {
                if let Err(err) = self.step(PipelineInput::Tick) {
                    error!(component = COMPONENT, error = %err, "pipeline housekeeping tick error");
                }
                next_tick = Instant::now() + HOUSEKEEPING_INTERVAL;
            }
        }

        info!(component = COMPONENT, "pipeline demo thread stopping");

        PipelineSummary {
            frames_processed: self.frames_processed,
            reader_overruns: self.reader_overruns,
            discontinuities: self.discontinuities,
        }
    }

    /// Spawns the dedicated `"pipeline-demo"` OS thread.
    #[cfg(feature = "demo")]
    pub fn spawn_demo(
        self,
        demo_rx: Receiver<Box<synthetic::SyntheticTick>>,
        cmd_rx: Receiver<PipelineCommand>,
        stop_flag: Arc<AtomicBool>,
    ) -> Result<JoinHandle<PipelineSummary>, std::io::Error> {
        Builder::new()
            .name("pipeline-demo".to_string())
            .spawn(move || self.run_demo(demo_rx, cmd_rx, stop_flag))
    }
}

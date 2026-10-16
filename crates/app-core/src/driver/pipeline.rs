//! The pipeline thread (`"pipeline"`, NORMAL priority).
//!
//! Owns the `Normalizer`. Input: `PipelineInput::{Lmu(Box<RawSnapshot>), Demo(Box<SyntheticTick>),
//! Command(PipelineCommand), Tick}`. Per frame it runs normalization, the [`super::engines`] `EngineHub`,
//! the app channels (`SRC_*`, `PIPELINE_LATENCY`, `net.*`, `rec.*`) and the `AlertEngine`, then encodes one
//! canonical TLV block shared as `Arc<[u8]>` and fans it out without blocking. Event-driven: no timer sits
//! in the 100 Hz path. A `recv_timeout(100ms)` expiry is the 10 Hz housekeeping tick (source-age ticker,
//! alert clock, recorder event drain, sleep/resume check).
//!
//! Implemented in app-core Phase 2 Increment I-3 (blueprint §2, §3, §4, §5, §6, §7, §12).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{Builder, JoinHandle};
use std::time::Duration;

use crossbeam_channel::Receiver;
use lmu_normalizer::{Normalizer, TelemetryDisposition};
use lmu_reader::{RawSnapshot, ReaderStats};
use protocol::messages::SchemaNegotiate;
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
    AlertDto, AlertLevel, AppMode, CoreStatus, DropsDto, ErrorDto, NetworkStatusDto, Origin,
    RatesDto, RecordingStatusDto, UiEvent,
};
use crate::clock::{AppClock, SkewStatus, SkewTracker};
use crate::driver::engines::{EngineHub, EngineHubConfig};
use crate::driver::net::NetPublisher;
use crate::driver::reader_thread::ReaderSnapshotMessage;
use crate::driver::recorder_sup::RecorderSupervisor;
use crate::driver::session::{
    DriverSessionMachine, DriverSessionSettings, SessionAction, SessionInputEvent,
};

/// Tracing component name for logging.
const COMPONENT: &str = "app_core::driver::pipeline";

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

            if let Some(ref mut net) = self.net {
                net.publish_scoring(scoring_state, snapshot.capture_mono_ns);
            }
            if let Some(ref mut rec) = self.recorder
                && let Ok(bytes) = postcard::to_stdvec(scoring_state)
            {
                let _ = rec.append_scoring(snapshot.capture_mono_ns, &bytes);
            }
            if let Some(ref sink) = self.sink
                && let Ok(v) = serde_json::to_value(scoring_state)
            {
                sink.emit(UiEvent::Scoring(v));
            }
        }

        // Identity updates:
        if let Some(ref identity) = output.identity {
            self.active_identity = Some(identity.clone());
        }

        // Telemetry frame processing:
        if output.disposition == TelemetryDisposition::NewSampleAfterRegression {
            self.discontinuities = self.discontinuities.saturating_add(1);
            self.discontinuity_pending = true;
            self.gap_before = true;
        }

        if let Some(mut frame) = output.frame {
            let now_mono_ns = self.clock.now_ns();
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
        }

        Ok(())
    }

    /// Processes the 100 ms timeout housekeeping tick (10 Hz).
    fn process_housekeeping_tick(&mut self) -> Result<(), PipelineError> {
        let now_mono_ns = self.clock.now_ns();

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
            let _ = rec.poll_events();
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
        if let Some(ref mut rec) = self.recorder
            && let Ok(bytes) = postcard::to_stdvec(event)
        {
            let _ = rec.append_session_event(mono_ns, &bytes);
        }

        // If this is a completed lap, also index it in the recorder:
        if let SessionEvent::LapCompleted { lap, time_s, valid } = event {
            let lap_rec = LapRecord {
                lap_number: *lap,
                stint_number: None,
                start_capture_ns: mono_ns,
                end_capture_ns: Some(mono_ns),
                lap_time_s: *time_s,
                sector_times_s: [None, None, None],
                valid: Some(*valid),
                pit_lap: None,
                metrics_json: None,
            };
            if let Some(ref mut rec) = self.recorder {
                let _ = rec.record_lap(lap_rec);
            }
        }

        if let Some(ref mut net) = self.net {
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

    /// Emits the composite `core:status` JSON event across the bridge.
    fn emit_status(&self, _now_mono_ns: u64) {
        if let Some(ref sink) = self.sink {
            let network_status = NetworkStatusDto {
                state: self.session_machine.connection_state().name().to_string(),
                rtt_ms: None,
                connect_code: None,
                code_expires_at_unix_s: None,
                engineers_connected: 0,
                engineers_max: 2,
                degraded_reason: None,
                driver_away: false,
                last_error: None,
                security_issue: false,
            };

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

            let net_frames_sent = self.net.as_ref().map(|n| {
                (n.stats().deltas_sent[0] + n.stats().deltas_sent[1] + n.stats().deltas_sent[2])
                    as f32
            });
            let net_drops = self.net.as_ref().map_or(0, |n| {
                n.stats().repair_pushes[0] + n.stats().repair_pushes[1] + n.stats().repair_pushes[2]
            });
            let net_dropped_obsolete = self
                .net
                .as_ref()
                .map_or(0, |n| n.stats().reliable_events_dropped);

            let recording_status = self
                .recorder
                .as_ref()
                .map(|r| r.status_dto())
                .unwrap_or_else(|| RecordingStatusDto {
                    state: "disabled".into(),
                    recording_id: None,
                    bytes_written: 0,
                    frames_written: 0,
                    frames_lost: 0,
                    disk_free_bytes: None,
                    error: None,
                });
            let recorder_overrun = self
                .recorder
                .as_ref()
                .map_or(0, |r| r.status_dto().frames_lost);

            let core_status = CoreStatus {
                mode: AppMode::Driver,
                origin: self.origin,
                lmu: self.session_machine.lmu_source_status(),
                session: self.session_machine.session_status_dto(),
                recording: recording_status,
                network: network_status,
                rates: RatesDto {
                    telemetry_read_hz: telem_rate,
                    scoring_read_hz: scoring_rate,
                    frames_sent_hz: net_frames_sent,
                    bytes_sent_per_s: None,
                    frames_dropped_total: net_drops,
                },
                drops: DropsDto {
                    reader_overrun: self.reader_overruns,
                    pipeline_overrun: self.pipeline_overruns,
                    recorder_overrun,
                    net_dropped_obsolete,
                    ui_dropped: self.ui_hub.total_ui_dropped(),
                },
            };

            sink.emit(UiEvent::Status(Box::new(core_status)));
        }
    }

    /// Processes an external pipeline command.
    fn process_command(&mut self, cmd: PipelineCommand) -> Result<(), PipelineError> {
        match cmd {
            PipelineCommand::SnapshotNeeded => {
                if let Some(ref mut net) = self.net {
                    net.request_snapshot();
                }
                self.discontinuity_pending = true;
            }
            PipelineCommand::StartStreaming => {
                let actions = self
                    .session_machine
                    .step(SessionInputEvent::ManualStartStreaming);
                self.execute_session_actions(actions);
            }
            PipelineCommand::StopStreaming => {
                let actions = self
                    .session_machine
                    .step(SessionInputEvent::ManualStopStreaming);
                self.execute_session_actions(actions);
            }
            PipelineCommand::EndSession => {
                let actions = self
                    .session_machine
                    .step(SessionInputEvent::ManualEndSession);
                self.execute_session_actions(actions);
                if let Some(ref mut rec) = self.recorder {
                    let _ = rec.on_session_end();
                }
            }
            PipelineCommand::SetRecording(enabled) => {
                if let Some(ref mut rec) = self.recorder {
                    let _ = rec.set_enabled(enabled);
                }
            }
            PipelineCommand::AddMarker(text) => {
                if let Some(ref mut rec) = self.recorder {
                    let _ = rec.append_marker(self.clock.now_ns(), &text);
                }
            }
            PipelineCommand::UpdateSettings(settings) => {
                let actions = self
                    .session_machine
                    .step(SessionInputEvent::SettingsUpdated(settings));
                self.execute_session_actions(actions);
            }
            PipelineCommand::Shutdown => {
                if let Some(ref mut rec) = self.recorder {
                    let _ = rec.on_session_end();
                }
            }
        }
        Ok(())
    }

    /// Executes discrete actions returned by the [`DriverSessionMachine`].
    fn execute_session_actions(&mut self, actions: Vec<SessionAction>) {
        for action in actions {
            match action {
                SessionAction::StartRecording(meta) => {
                    if let Some(ref mut rec) = self.recorder
                        && let Err(err) = rec.on_session_ready(meta)
                    {
                        warn!(
                            component = COMPONENT,
                            error = %err,
                            "failed to start local recording session"
                        );
                    }
                }
                SessionAction::FinishRecordingSegment | SessionAction::FinishRecordingSession => {
                    if let Some(ref mut rec) = self.recorder {
                        let _ = rec.on_session_stalled();
                    }
                }
                SessionAction::ResumeRecordingSegment => {
                    // Segment resumes automatically on next appended frame
                }
                SessionAction::StartStreaming => {
                    info!(
                        component = COMPONENT,
                        "live streaming requested by session machine"
                    );
                }
                SessionAction::StopStreaming => {
                    info!(
                        component = COMPONENT,
                        "live streaming paused by session machine"
                    );
                }
                SessionAction::EndStreamingSession => {
                    info!(component = COMPONENT, "live streaming session ended");
                }
                SessionAction::SendStaleBlock => {
                    self.emit_stale_block();
                }
                SessionAction::SendSchemaNegotiate => {
                    if let Some(ref mut net) = self.net {
                        let schema = SchemaNegotiate {
                            schema_version: TELEMETRY_SCHEMA_VERSION,
                            capabilities: self.normalizer.capability_map().clone(),
                            vehicle_capabilities: self.normalizer.vehicle_capabilities(),
                        };
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
                        let is_shutdown = matches!(cmd, PipelineCommand::Shutdown);
                        if let Err(err) = self.step(PipelineInput::Command(cmd)) {
                            error!(component = COMPONENT, error = %err, "pipeline error processing command");
                        }
                        if is_shutdown {
                            break;
                        }
                    }
                }
                default(Duration::from_millis(100)) => {
                    if let Err(err) = self.step(PipelineInput::Tick) {
                        error!(component = COMPONENT, error = %err, "pipeline housekeeping tick error");
                    }
                }
            }
        }

        info!(
            component = COMPONENT,
            "pipeline thread stopping; finalizing recording"
        );
        if let Some(ref mut rec) = self.recorder {
            let _ = rec.on_session_end();
        }

        PipelineSummary {
            frames_processed: self.frames_processed,
            reader_overruns: self.reader_overruns,
            discontinuities: self.discontinuities,
        }
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
        let latency_s = self.pipeline_latency_ns as f64 / 1_000_000_000.0;
        let _ = frame.set_valid(ch::PIPELINE_LATENCY, Value::F64(latency_s));
        let _ = frame.set_valid(ch::SRC_TELEMETRY_AGE, Value::F64(0.0));
        let scoring_age_s = self.last_scoring_change_mono_ns.map_or(0.0, |t| {
            (frame.capture_mono_ns.saturating_sub(t) as f64) / 1e9
        });
        let _ = frame.set_valid(ch::SRC_SCORING_AGE, Value::F64(scoring_age_s));
        let _ = frame.set_valid(ch::NORMALIZER_FORMULA_VERSION, Value::U32(1));

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
                    if let Ok(cmd) = msg {
                        let is_shutdown = matches!(cmd, PipelineCommand::Shutdown);
                        if let Err(err) = self.step(PipelineInput::Command(cmd)) {
                            error!(component = COMPONENT, error = %err, "pipeline error processing command");
                        }
                        if is_shutdown {
                            break;
                        }
                    }
                }
                default(Duration::from_millis(100)) => {
                    if let Err(err) = self.step(PipelineInput::Tick) {
                        error!(component = COMPONENT, error = %err, "pipeline housekeeping tick error");
                    }
                }
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

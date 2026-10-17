//! `NetPublisher`: driver telemetry and event publisher to the relay (blueprint §1 Decision 4, §3, §12 Lane D, §14).
//!
//! # Architecture & Responsibilities
//! - **Schema negotiation first**: Sends [`SchemaNegotiate`] first in each sequence space (resets on session
//!   restart or identity change).
//! - **Priority splitting (P0, P1, P2)**: Encodes changed channels into separate deltas per priority class
//!   ([`FramePriority::P0`], [`FramePriority::P1`], [`FramePriority::P2`]).
//! - **Union repair (Decision 4)**: If `send_live` reports `LiveOutcome.replaced == true` (an unsent older delta
//!   was overwritten by latest-wins in that slot), the sender immediately re-pushes with
//!   `changed(i) ∪ channel-set(previous push)` to prevent channel loss.
//! - **Periodic & On-demand Snapshots**: Emits a full [`TelemetrySnapshot`] at 1 Hz, and immediately upon
//!   receiving an on-demand snapshot request.
//! - **Scoring & Weather**: Emits [`ScoringUpdate`] at 5 Hz, and [`WeatherUpdate`] on change at ≤ 1 Hz.
//!   Environment-class channels are excluded from telemetry deltas.
//! - **Driver and Recording Status**: Emits [`DriverStatus`] and [`RecordingStatus`] at 1 Hz.
//! - **Reliable Queue Management**: Bounded buffer (capacity 1024) for [`SessionEventMsg`], dropping the oldest
//!   when congested and counting drops. Keeps only the newest [`SchemaNegotiate`].
//! - **No Synthetic Data**: Rejects or ignores synthetic data (owner decision Q1). Native 100 Hz for all
//!   priorities (owner decision Q2).

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use protocol::clock::ClockEstimate;
use protocol::frame::FramePriority;
use protocol::messages::{
    CaptureTime, ConnectCode, DriverStatus, EngineerEntry, Message, RecordingStatus,
    SchemaNegotiate, ScoringUpdate, SessionCreate, SessionEventMsg, TelemetryPayload,
};
use telemetry_model::channel::{ChannelId, FrequencyClass, Priority};
use telemetry_model::codec::{self, RELAY_NS_UNKNOWN, TlvEncoder, TlvHeader};
use telemetry_model::events::EventRecord;
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::registry;
use telemetry_model::scoring::ScoringState;
use telemetry_model::value::Sample;
use transport::driver::DriverClient;
use transport::metrics::LinkMetrics;
use transport::queue::{LiveOutcome, QueueError};
use transport::state::ConnectionStatus;

use crate::clock::AppClock;

/// Maximum number of queued reliable session events before dropping the oldest.
pub const MAX_PENDING_SESSION_EVENTS: usize = 1024;

/// Interval between periodic full telemetry snapshots (1 Hz = 1 second).
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(1);

/// Minimum interval between scoring updates (5 Hz = 200 ms).
pub const SCORING_INTERVAL: Duration = Duration::from_millis(200);

/// Minimum interval between weather updates (≤ 1 Hz = 1 second).
pub const WEATHER_INTERVAL: Duration = Duration::from_secs(1);

/// Minimum interval between driver/recording status broadcasts (1 Hz = 1 second).
pub const STATUS_INTERVAL: Duration = Duration::from_secs(1);

/// Abstraction over the driver network transport sink (blueprint §3, §12).
///
/// Permits headless testing with mock sinks that simulate replaced latest-wins slots,
/// queue backpressure, and transport disconnection.
pub trait LiveSink: Send + Sync {
    /// Queues a latest-wins live frame.
    fn send_live(
        &self,
        message: Message,
        priority: FramePriority,
    ) -> Result<LiveOutcome, QueueError>;

    /// Tries to queue a reliable control/stream message without blocking.
    fn try_send_reliable(&self, message: Message) -> Result<(), QueueError>;

    /// Current connection status.
    fn connection_status(&self) -> ConnectionStatus;

    /// Estimates relay time for a given capture instant.
    fn relay_time_of(&self, at: Instant) -> Option<i64>;

    /// Full link state for status reporting (connection status, metrics, connect code and the
    /// engineers holding a slot). `None` when the sink cannot report it (e.g. test mocks): the
    /// status then shows the link as unknown, never as healthy.
    fn link_snapshot(&self) -> Option<LinkSnapshot> {
        None
    }
}

/// Point-in-time view of the driver's relay link (D-022 controls and health).
#[derive(Debug, Clone, PartialEq)]
pub struct LinkSnapshot {
    /// Transport connection status (state, failure, degraded reason, driver away, last error).
    pub status: ConnectionStatus,
    /// Link metrics (RTT, rates, drops).
    pub metrics: LinkMetrics,
    /// Current one-time connection code and slot usage (`None` while no session is bound).
    pub connect_code: Option<ConnectCode>,
    /// Engineers holding a slot in the session.
    pub engineers: Vec<EngineerEntry>,
    /// Clock offset to the relay, once measured.
    pub clock: Option<ClockEstimate>,
}

/// Why a stream control request could not be queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StreamControlError {
    /// The bounded control queue is full (the transport is busy); retry later.
    #[error("the relay control queue is full")]
    Busy,
    /// The relay client has shut down.
    #[error("the relay client has shut down")]
    Closed,
}

/// Non-blocking control of the driver's relay session (start/stop/end). Implementations queue the
/// request and return at once, so the pipeline thread never waits on the network.
pub trait StreamControl: Send + Sync {
    /// Connects and binds (resumes or creates) the relay session, then streams.
    fn start(&self, session: SessionCreate) -> Result<(), StreamControlError>;
    /// Updates the SESSION_CREATE data used for the next new session (the bound one is kept, Q3).
    fn update_session(&self, session: SessionCreate) -> Result<(), StreamControlError>;
    /// Stops streaming and disconnects, keeping the relay session for a resume.
    fn stop(&self) -> Result<(), StreamControlError>;
    /// Ends the relay session (engineers see "driver ended the session").
    fn end_session(&self) -> Result<(), StreamControlError>;
}

impl LiveSink for DriverClient {
    fn send_live(
        &self,
        message: Message,
        priority: FramePriority,
    ) -> Result<LiveOutcome, QueueError> {
        self.send_live(message, priority)
    }

    fn try_send_reliable(&self, message: Message) -> Result<(), QueueError> {
        self.try_send_reliable(message)
    }

    fn connection_status(&self) -> ConnectionStatus {
        self.status().borrow().clone()
    }

    fn relay_time_of(&self, at: Instant) -> Option<i64> {
        self.relay_time_of(at)
    }

    fn link_snapshot(&self) -> Option<LinkSnapshot> {
        let metrics = self.metrics().borrow().clone();
        Some(LinkSnapshot {
            status: self.status().borrow().clone(),
            clock: metrics.clock,
            metrics,
            connect_code: self.connect_code().borrow().clone(),
            engineers: self.engineers().borrow().clone(),
        })
    }
}

/// Diagnostic throughput and queue counters for [`NetPublisher`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetPublisherStats {
    pub frames_processed: u64,
    pub deltas_sent: [u64; 3],
    pub snapshots_sent: u64,
    pub scorings_sent: u64,
    pub weathers_sent: u64,
    pub statuses_sent: u64,
    pub repair_pushes: [u64; 3],
    pub reliable_events_sent: u64,
    pub reliable_events_dropped: u64,
}

/// Converts a registry [`Priority`] into the corresponding protocol [`FramePriority`].
pub const fn to_frame_priority(p: Priority) -> FramePriority {
    match p {
        Priority::P0 => FramePriority::P0,
        Priority::P1 => FramePriority::P1,
        Priority::P2 => FramePriority::P2,
    }
}

/// Converts a protocol [`FramePriority`] index (0, 1, 2) to its priority value.
pub const fn priority_index(p: FramePriority) -> usize {
    match p {
        FramePriority::P0 => 0,
        FramePriority::P1 => 1,
        FramePriority::P2 => 2,
        _ => 0,
    }
}

/// Driver network stream publisher.
pub struct NetPublisher {
    sink: Arc<dyn LiveSink>,
    /// Relay session control (start/stop/end); `None` for publishers without one (tests).
    control: Option<Arc<dyn StreamControl>>,
    clock: AppClock,
    baseline_frame: Option<TelemetryFrame>,
    /// Accumulated unacknowledged/replaced channel sets per priority (0: P0, 1: P1, 2: P2).
    pending_channel_sets: [BTreeSet<ChannelId>; 3],
    /// Pending schema negotiation waiting to be sent reliably.
    pending_schema: Option<SchemaNegotiate>,
    /// Pending reliable session events queue.
    pending_session_events: VecDeque<SessionEventMsg>,
    /// Timestamp of the last full snapshot sent (mono ns).
    last_snapshot_mono_ns: Option<u64>,
    /// Flag indicating that an on-demand snapshot was requested.
    snapshot_requested: bool,
    /// Timestamp of the last scoring update sent (mono ns).
    last_scoring_mono_ns: Option<u64>,
    /// Timestamp of the last weather update sent (mono ns).
    last_weather_mono_ns: Option<u64>,
    /// Last published weather channel samples to detect change.
    last_weather_values: Option<Vec<(ChannelId, Sample)>>,
    /// Timestamp of the last status broadcast (mono ns).
    last_status_mono_ns: Option<u64>,
    /// Internal statistics.
    stats: NetPublisherStats,
}

impl NetPublisher {
    /// Creates a new network publisher with the given transport sink and clock.
    pub fn new(sink: Arc<dyn LiveSink>, clock: AppClock) -> Self {
        Self {
            sink,
            control: None,
            clock,
            baseline_frame: None,
            pending_channel_sets: [BTreeSet::new(), BTreeSet::new(), BTreeSet::new()],
            pending_schema: None,
            pending_session_events: VecDeque::with_capacity(64),
            last_snapshot_mono_ns: None,
            snapshot_requested: false,
            last_scoring_mono_ns: None,
            last_weather_mono_ns: None,
            last_weather_values: None,
            last_status_mono_ns: None,
            stats: NetPublisherStats::default(),
        }
    }

    /// Attaches the relay session control used for start/stop/end of streaming.
    pub fn with_control(mut self, control: Arc<dyn StreamControl>) -> Self {
        self.control = Some(control);
        self
    }

    /// The relay session control, if attached.
    pub fn control(&self) -> Option<&Arc<dyn StreamControl>> {
        self.control.as_ref()
    }

    /// Current link state of the sink (see [`LiveSink::link_snapshot`]).
    pub fn link_snapshot(&self) -> Option<LinkSnapshot> {
        self.sink.link_snapshot()
    }

    /// Forgets the delta baseline so the next published frame is a full snapshot (a new relay
    /// session or sequence space starts).
    pub fn reset_stream(&mut self) {
        self.baseline_frame = None;
        self.snapshot_requested = true;
        for set in &mut self.pending_channel_sets {
            set.clear();
        }
    }

    /// Returns a reference to diagnostic statistics.
    pub fn stats(&self) -> &NetPublisherStats {
        &self.stats
    }

    /// Sets or updates the active schema negotiation message.
    ///
    /// This is queued to be sent reliably at the beginning of a sequence space.
    pub fn set_schema(&mut self, schema: SchemaNegotiate) {
        self.pending_schema = Some(schema);
        self.drain_reliable();
    }

    /// Requests an immediate full snapshot on the next frame (e.g. from engineer request).
    pub fn request_snapshot(&mut self) {
        self.snapshot_requested = true;
    }

    /// Queues a reliable session event for transmission.
    pub fn push_session_event(&mut self, record: EventRecord, mono_ns: u64) {
        let relay_ns_est = self.relay_ns_at(mono_ns);
        let capture = CaptureTime {
            mono_ns,
            relay_ns_est: if relay_ns_est == RELAY_NS_UNKNOWN {
                None
            } else {
                Some(relay_ns_est)
            },
        };
        let msg = SessionEventMsg { capture, record };

        if self.pending_session_events.len() >= MAX_PENDING_SESSION_EVENTS {
            self.pending_session_events.pop_front();
            self.stats.reliable_events_dropped =
                self.stats.reliable_events_dropped.saturating_add(1);
        }
        self.pending_session_events.push_back(msg);
        self.drain_reliable();
    }

    /// Attempts to drain pending reliable messages (SchemaNegotiate and SessionEvents).
    pub fn drain_reliable(&mut self) {
        // 1. Drain schema negotiation if pending
        if let Some(ref schema) = self.pending_schema {
            match self
                .sink
                .try_send_reliable(Message::SchemaNegotiate(schema.clone()))
            {
                Ok(()) => {
                    self.pending_schema = None;
                }
                Err(QueueError::Full { .. }) => {
                    // Queue full; wait for subsequent ticks
                    return;
                }
                Err(QueueError::NotStreaming | QueueError::Closed) => {
                    // Not streaming or closed; leave pending until streaming resumes
                    return;
                }
                Err(_) => {
                    // Unrecoverable queue error for this message
                    self.pending_schema = None;
                }
            }
        }

        // 2. Drain session events in FIFO order
        while let Some(event) = self.pending_session_events.front() {
            match self
                .sink
                .try_send_reliable(Message::SessionEvent(event.clone()))
            {
                Ok(()) => {
                    self.pending_session_events.pop_front();
                    self.stats.reliable_events_sent =
                        self.stats.reliable_events_sent.saturating_add(1);
                }
                Err(QueueError::Full { .. }) => {
                    break;
                }
                Err(QueueError::NotStreaming | QueueError::Closed) => {
                    break;
                }
                Err(_) => {
                    self.pending_session_events.pop_front();
                }
            }
        }
    }

    /// Estimates relay time for a given capture timestamp in monotonic nanoseconds.
    pub fn relay_ns_at(&self, mono_ns: u64) -> i64 {
        let epoch = self.clock.epoch();
        let instant = epoch
            .checked_add(Duration::from_nanos(mono_ns))
            .unwrap_or(epoch);
        self.sink.relay_time_of(instant).unwrap_or(RELAY_NS_UNKNOWN)
    }

    /// Evaluates and streams scoring updates at at most 5 Hz.
    pub fn publish_scoring(&mut self, scoring: &ScoringState, mono_ns: u64) {
        let should_send = match self.last_scoring_mono_ns {
            Some(last) => mono_ns.saturating_sub(last) >= SCORING_INTERVAL.as_nanos() as u64,
            None => true,
        };

        if should_send {
            let relay_ns_est = self.relay_ns_at(mono_ns);
            let capture = CaptureTime {
                mono_ns,
                relay_ns_est: if relay_ns_est == RELAY_NS_UNKNOWN {
                    None
                } else {
                    Some(relay_ns_est)
                },
            };
            let update = ScoringUpdate {
                capture,
                scoring: scoring.clone(),
            };

            if self
                .sink
                .send_live(Message::ScoringUpdate(update), FramePriority::P0)
                .is_ok()
            {
                self.last_scoring_mono_ns = Some(mono_ns);
                self.stats.scorings_sent = self.stats.scorings_sent.saturating_add(1);
            }
        }
    }

    /// Evaluates and streams weather updates on change at at most 1 Hz.
    ///
    /// Weather updates contain only channels with [`FrequencyClass::Environment`].
    pub fn publish_weather(&mut self, frame: &TelemetryFrame, mono_ns: u64) {
        let env_channels: Vec<(ChannelId, Sample)> = frame
            .iter()
            .filter(|(def, _)| def.frequency == FrequencyClass::Environment)
            .map(|(def, sample)| (def.id, sample.clone()))
            .collect();

        if env_channels.is_empty() {
            return;
        }

        let has_changed = match self.last_weather_values {
            Some(ref prev) => {
                prev.len() != env_channels.len()
                    || prev
                        .iter()
                        .zip(&env_channels)
                        .any(|((id1, s1), (id2, s2))| id1 != id2 || s1 != s2)
            }
            None => true,
        };

        let rate_allowed = match self.last_weather_mono_ns {
            Some(last) => mono_ns.saturating_sub(last) >= WEATHER_INTERVAL.as_nanos() as u64,
            None => true,
        };

        if has_changed && rate_allowed {
            let relay_ns_est = self.relay_ns_at(mono_ns);
            let header = TlvHeader {
                schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
                capture_mono_ns: mono_ns,
                capture_relay_ns_est: relay_ns_est,
                source_et_s: frame.source_et_s.unwrap_or(f64::NAN),
            };

            let mut buf = Vec::with_capacity(codec::TLV_HEADER_BYTES + env_channels.len() * 12);
            let mut enc = TlvEncoder::begin(&mut buf, &header);
            for (id, sample) in &env_channels {
                let _ = enc.push(*id, sample);
            }
            enc.finish();

            if let Ok(payload) = TelemetryPayload::new(Bytes::from(buf))
                && self
                    .sink
                    .send_live(Message::WeatherUpdate(payload), FramePriority::P2)
                    .is_ok()
            {
                self.last_weather_mono_ns = Some(mono_ns);
                self.last_weather_values = Some(env_channels);
                self.stats.weathers_sent = self.stats.weathers_sent.saturating_add(1);
            }
        }
    }

    /// Evaluates and streams driver and recording status at 1 Hz.
    pub fn publish_status(
        &mut self,
        driver_status: DriverStatus,
        recording_status: RecordingStatus,
        mono_ns: u64,
    ) {
        let should_send = match self.last_status_mono_ns {
            Some(last) => mono_ns.saturating_sub(last) >= STATUS_INTERVAL.as_nanos() as u64,
            None => true,
        };

        if should_send {
            let mut sent_any = false;
            if self
                .sink
                .send_live(Message::DriverStatus(driver_status), FramePriority::P0)
                .is_ok()
            {
                sent_any = true;
            }
            if self
                .sink
                .send_live(
                    Message::RecordingStatus(recording_status),
                    FramePriority::P0,
                )
                .is_ok()
            {
                sent_any = true;
            }
            if sent_any {
                self.last_status_mono_ns = Some(mono_ns);
                self.stats.statuses_sent = self.stats.statuses_sent.saturating_add(1);
            }
        }
    }

    /// Main frame processing entry point.
    ///
    /// Encodes and publishes full snapshot (1 Hz or on request) or per-priority deltas (with union repair).
    pub fn publish_frame(&mut self, frame: &TelemetryFrame, mono_ns: u64) {
        self.stats.frames_processed = self.stats.frames_processed.saturating_add(1);

        // 1. Drain pending reliable control/event messages
        self.drain_reliable();

        // 2. Check snapshot requirement
        let needs_snapshot = self.snapshot_requested
            || self.baseline_frame.is_none()
            || self.last_snapshot_mono_ns.is_none_or(|last| {
                mono_ns.saturating_sub(last) >= SNAPSHOT_INTERVAL.as_nanos() as u64
            });

        if needs_snapshot {
            self.send_snapshot(frame, mono_ns);
            return;
        }

        // 3. Delta encoding with priority splitting and union repair
        let baseline = self.baseline_frame.clone();
        if let Some(ref baseline_f) = baseline {
            self.send_priority_deltas(frame, baseline_f, mono_ns);
        }

        self.baseline_frame = Some(frame.clone());
    }

    fn send_snapshot(&mut self, frame: &TelemetryFrame, mono_ns: u64) {
        let relay_ns_est = self.relay_ns_at(mono_ns);

        let mut buf = Vec::with_capacity(4096);
        if frame.encode_snapshot(&mut buf, relay_ns_est).is_ok()
            && let Ok(payload) = TelemetryPayload::new(Bytes::from(buf))
            && self
                .sink
                .send_live(Message::TelemetrySnapshot(payload), FramePriority::P0)
                .is_ok()
        {
            self.last_snapshot_mono_ns = Some(mono_ns);
            self.snapshot_requested = false;
            self.stats.snapshots_sent = self.stats.snapshots_sent.saturating_add(1);

            // A snapshot supersedes all pending latest-wins deltas across all priorities
            for set in &mut self.pending_channel_sets {
                set.clear();
            }
            self.baseline_frame = Some(frame.clone());
        }
    }

    fn send_priority_deltas(
        &mut self,
        current: &TelemetryFrame,
        baseline: &TelemetryFrame,
        mono_ns: u64,
    ) {
        let relay_ns_est = self.relay_ns_at(mono_ns);

        // Group changed channels by priority (excluding Environment frequency channels)
        let mut p0_changed = BTreeSet::new();
        let mut p1_changed = BTreeSet::new();
        let mut p2_changed = BTreeSet::new();

        for channel_id in current.changed_channels(baseline) {
            if let Some(def) = registry::by_id(channel_id)
                && def.frequency != FrequencyClass::Environment
            {
                match def.priority {
                    Priority::P0 => {
                        p0_changed.insert(channel_id);
                    }
                    Priority::P1 => {
                        p1_changed.insert(channel_id);
                    }
                    Priority::P2 => {
                        p2_changed.insert(channel_id);
                    }
                }
            }
        }

        let priorities = [
            (FramePriority::P0, p0_changed),
            (FramePriority::P1, p1_changed),
            (FramePriority::P2, p2_changed),
        ];

        for (fprio, changed_set) in priorities {
            let idx = priority_index(fprio);

            if changed_set.is_empty() {
                continue;
            }

            let delta_payload =
                self.encode_channel_set(current, &changed_set, mono_ns, relay_ns_est);
            if let Some(payload) = delta_payload {
                match self.sink.send_live(Message::TelemetryDelta(payload), fprio) {
                    Ok(outcome) => {
                        self.stats.deltas_sent[idx] = self.stats.deltas_sent[idx].saturating_add(1);

                        if outcome.replaced {
                            // Union repair (Decision 4):
                            // The frame we just pushed replaced an unsent older frame in this slot!
                            // Any channels present in that replaced frame must be preserved in the queue.
                            // We immediately re-push with `changed(i) ∪ channel-set(previous push)`.
                            let mut union_set = changed_set.clone();
                            for id in &self.pending_channel_sets[idx] {
                                union_set.insert(*id);
                            }

                            if union_set != changed_set
                                && let Some(union_payload) = self.encode_channel_set(
                                    current,
                                    &union_set,
                                    mono_ns,
                                    relay_ns_est,
                                )
                            {
                                let _ = self
                                    .sink
                                    .send_live(Message::TelemetryDelta(union_payload), fprio);
                                self.stats.repair_pushes[idx] =
                                    self.stats.repair_pushes[idx].saturating_add(1);
                            }
                            self.pending_channel_sets[idx] = union_set;
                        } else {
                            // Successfully queued into an empty slot (no overwrite).
                            self.pending_channel_sets[idx] = changed_set;
                        }
                    }
                    Err(_) => {
                        // Queue error; retain channel set for next frame attempt
                        self.pending_channel_sets[idx] = changed_set;
                    }
                }
            }
        }
    }

    fn encode_channel_set(
        &self,
        frame: &TelemetryFrame,
        channels: &BTreeSet<ChannelId>,
        mono_ns: u64,
        relay_ns_est: i64,
    ) -> Option<TelemetryPayload> {
        if channels.is_empty() {
            return None;
        }

        let header = TlvHeader {
            schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
            capture_mono_ns: mono_ns,
            capture_relay_ns_est: relay_ns_est,
            source_et_s: frame.source_et_s.unwrap_or(f64::NAN),
        };

        let mut buf = Vec::with_capacity(codec::TLV_HEADER_BYTES + channels.len() * 12);
        let mut enc = TlvEncoder::begin(&mut buf, &header);
        for id in channels {
            let sample = frame.get(*id);
            let _ = enc.push(*id, sample);
        }
        enc.finish();

        TelemetryPayload::new(Bytes::from(buf)).ok()
    }
}

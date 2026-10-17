//! `EngineerCore`: consumer of the engineer stream from the relay.
//!
//! Consumes [`EngineerEvent`]s without blocking (byte copy and queue push only).
//! Enforces the schema guard, coalesces per-priority deltas of one capture
//! (equal `capture_mono_ns`, flushed on change or after 8 ms) into one block for [`UiHub`],
//! preserves pre-gap history across blips with a visible gap flag (owner decision Q4),
//! and tracks stream freshness via [`StreamAgeTracker`].

use std::collections::VecDeque;
use std::sync::Arc;

use protocol::messages::{
    ConnectCodeError, DriverPresence, DriverStatus, ErrorCode, Message, RecordingStatus,
    SchemaNegotiate, ScoringUpdate, SessionEventMsg, normalize_connect_code,
};
use race_engines::alert::{AlertConfig, AlertEngine, AlertNotice};
use telemetry_model::TELEMETRY_SCHEMA_VERSION;
use telemetry_model::codec::TLV_HEADER_BYTES;
use transport::engineer::{EngineerEvent, IdentityCard};
use transport::state::EngineerEndReason;

use crate::bridge::hub::UiHub;
use crate::engineer::age::{StreamAgeReport, StreamAgeTracker, StreamHealth};

/// Timeout before an uncompleted coalesced capture is flushed to UI (8 ms).
pub const COALESCE_TIMEOUT_NS: u64 = 8_000_000;

/// Bounded queue capacity for session events retained in memory on the engineer side.
pub const MAX_ENGINEER_SESSION_EVENTS: usize = 1024;

/// Diagnostic statistics of `EngineerCore`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineerCoreStats {
    pub snapshots_received: u64,
    pub deltas_received: u64,
    pub coalesced_merges: u64,
    pub blocks_flushed: u64,
    pub schema_mismatches: u64,
    pub stream_resets: u64,
}

/// Composite status snapshot of the engineer stream and session.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineerStatusSnapshot {
    pub identity: Option<IdentityCard>,
    pub stream_health: StreamHealth,
    pub stream_age_s: Option<f64>,
    pub driver_away: bool,
    pub ended_reason: Option<EngineerEndReason>,
    pub incompatible_schema: bool,
    pub active_alerts: usize,
}

/// Output of a 4 Hz engineer evaluation tick.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineerTickOutput {
    pub status: EngineerStatusSnapshot,
    pub alerts: Vec<AlertNotice>,
}

/// Engineer core processing engine.
pub struct EngineerCore {
    hub: Arc<UiHub>,
    age_tracker: StreamAgeTracker,
    alert_engine: Option<AlertEngine>,

    // Coalescer state
    current_capture_mono_ns: Option<u64>,
    coalesced_buf: Vec<u8>,
    last_received_mono_ns: u64,
    gap_pending: bool,

    // Session and stream state
    active_identity: Option<IdentityCard>,
    ended_reason: Option<EngineerEndReason>,
    driver_away: Option<(u64, u64)>,
    incompatible_schema: bool,
    schema: Option<SchemaNegotiate>,
    latest_scoring: Option<ScoringUpdate>,
    latest_driver_status: Option<DriverStatus>,
    latest_recording_status: Option<RecordingStatus>,
    driver_presence: Option<DriverPresence>,
    latest_relay_error: Option<(ErrorCode, String)>,
    session_events: VecDeque<SessionEventMsg>,
    last_status: Option<EngineerStatusSnapshot>,

    stats: EngineerCoreStats,
}

impl EngineerCore {
    /// Creates a new `EngineerCore` feeding the provided [`UiHub`].
    pub fn new(hub: Arc<UiHub>) -> Self {
        let alert_engine = AlertEngine::new(&AlertConfig::default()).ok();

        Self {
            hub,
            age_tracker: StreamAgeTracker::new(),
            alert_engine,
            current_capture_mono_ns: None,
            coalesced_buf: Vec::with_capacity(4096),
            last_received_mono_ns: 0,
            gap_pending: false,
            active_identity: None,
            ended_reason: None,
            driver_away: None,
            incompatible_schema: false,
            schema: None,
            latest_scoring: None,
            latest_driver_status: None,
            latest_recording_status: None,
            driver_presence: None,
            latest_relay_error: None,
            session_events: VecDeque::with_capacity(64),
            last_status: None,
            stats: EngineerCoreStats::default(),
        }
    }

    /// Access diagnostic statistics.
    pub fn stats(&self) -> &EngineerCoreStats {
        &self.stats
    }

    /// Returns the currently active session identity card, if joined.
    pub fn active_identity(&self) -> Option<&IdentityCard> {
        self.active_identity.as_ref()
    }

    /// Returns the reason why the engineer connection ended, if ended.
    pub fn ended_reason(&self) -> Option<&EngineerEndReason> {
        self.ended_reason.as_ref()
    }

    /// Returns the latest scoring update, if available.
    pub fn latest_scoring(&self) -> Option<&ScoringUpdate> {
        self.latest_scoring.as_ref()
    }

    /// The driver's latest DRIVER_STATUS (LMU state, layered health, rates), if received.
    pub fn latest_driver_status(&self) -> Option<&DriverStatus> {
        self.latest_driver_status.as_ref()
    }

    /// The driver's latest RECORDING_STATUS, if received.
    pub fn latest_recording_status(&self) -> Option<&RecordingStatus> {
        self.latest_recording_status.as_ref()
    }

    /// Status computed by the most recent [`EngineerCore::tick`] (`None` before the first tick).
    pub fn last_status(&self) -> Option<&EngineerStatusSnapshot> {
        self.last_status.as_ref()
    }

    /// Whether the driver is currently away from the relay session (DRIVER_PRESENCE, protocol 1.1).
    pub fn driver_away(&self) -> bool {
        self.driver_away.is_some() || self.driver_presence.as_ref().is_some_and(|p| !p.present)
    }

    /// Validates code formatting before passing to `EngineerClient::join`.
    ///
    /// Architectural invariant: An engineer never auto-connects without an explicit user code.
    pub fn validate_connect_code(code: &str) -> Result<String, ConnectCodeError> {
        normalize_connect_code(code).map(|s| s.expose().to_owned())
    }

    /// Non-blocking event handler: processes one [`EngineerEvent`].
    pub fn handle_event(&mut self, event: EngineerEvent, now_mono_ns: u64) {
        match event {
            EngineerEvent::Frame(inbound) => {
                let received_at_ns = inbound.received_at_ns;
                match inbound.message {
                    Message::TelemetrySnapshot(payload) => {
                        self.process_tlv(payload.as_bytes(), true, received_at_ns);
                    }
                    Message::TelemetryDelta(payload) => {
                        self.process_tlv(payload.as_bytes(), false, received_at_ns);
                    }
                    Message::WeatherUpdate(payload) => {
                        self.process_tlv(payload.as_bytes(), false, received_at_ns);
                    }
                    Message::ScoringUpdate(scoring) => {
                        self.flush();
                        self.latest_scoring = Some(scoring);
                    }
                    Message::DriverStatus(status) => {
                        if let Some(ref clock) = status.clock {
                            self.age_tracker
                                .record_driver_clock(clock.offset_ns, clock.uncertainty_ns);
                        }
                        self.latest_driver_status = Some(status);
                    }
                    Message::RecordingStatus(status) => {
                        self.latest_recording_status = Some(status);
                    }
                    Message::SessionEvent(event_msg) => {
                        if self.session_events.len() >= MAX_ENGINEER_SESSION_EVENTS {
                            self.session_events.pop_front();
                        }
                        self.session_events.push_back(event_msg);
                    }
                    Message::DriverPresence(presence) => {
                        self.driver_presence = Some(presence);
                    }
                    Message::SchemaNegotiate(schema) => {
                        if schema.schema_version != TELEMETRY_SCHEMA_VERSION {
                            self.stats.schema_mismatches =
                                self.stats.schema_mismatches.saturating_add(1);
                            self.incompatible_schema = true;
                        } else {
                            self.schema = Some(schema);
                        }
                    }
                    _ => {}
                }
            }
            EngineerEvent::StreamReset { card, gap: _ } => {
                self.flush();
                // Owner decision Q4: preserve pre-gap history in UiHub across blips,
                // but signal a visible gap via gap_pending.
                self.gap_pending = true;
                self.active_identity = Some(card);
                self.age_tracker.reset_stream();
                self.stats.stream_resets = self.stats.stream_resets.saturating_add(1);
            }
            EngineerEvent::Ended(reason) => {
                self.flush();
                self.ended_reason = Some(reason);
            }
            EngineerEvent::DriverAway {
                since_ns,
                grace_expires_at_ns,
            } => {
                self.driver_away = Some((since_ns, grace_expires_at_ns));
            }
            EngineerEvent::DriverBack => {
                self.driver_away = None;
            }
            EngineerEvent::RelayError { code, message } => {
                self.latest_relay_error = Some((code, message));
            }
        }

        // Check coalescer timeout: flush if held for longer than 8 ms
        if self.current_capture_mono_ns.is_some()
            && now_mono_ns.saturating_sub(self.last_received_mono_ns) >= COALESCE_TIMEOUT_NS
        {
            self.flush();
        }
    }

    /// Processes an inbound TLV byte block (snapshot or delta).
    fn process_tlv(&mut self, block: &[u8], is_snapshot: bool, received_at_ns: u64) {
        if block.len() < TLV_HEADER_BYTES {
            return;
        }

        // 1. Schema Guard: verify schema version in header
        let schema_version = u16::from_le_bytes([block[0], block[1]]);
        if schema_version != TELEMETRY_SCHEMA_VERSION {
            self.stats.schema_mismatches = self.stats.schema_mismatches.saturating_add(1);
            self.incompatible_schema = true;
            return;
        }

        // 2. Parse capture timestamps
        let capture_mono_ns = u64::from_le_bytes([
            block[2], block[3], block[4], block[5], block[6], block[7], block[8], block[9],
        ]);
        let capture_relay_ns_est = i64::from_le_bytes([
            block[10], block[11], block[12], block[13], block[14], block[15], block[16], block[17],
        ]);

        self.age_tracker
            .record_frame(capture_mono_ns, capture_relay_ns_est, received_at_ns);

        // 3. Snapshot handling (keyframes are pushed immediately)
        if is_snapshot {
            self.flush();
            let arc_block: Arc<[u8]> = Arc::from(block);
            self.hub
                .push_block_with_gap(arc_block, true, self.gap_pending);
            self.gap_pending = false;
            self.stats.snapshots_received = self.stats.snapshots_received.saturating_add(1);
            return;
        }

        // 4. Delta coalescing per equal `capture_mono_ns`
        if self.current_capture_mono_ns == Some(capture_mono_ns) {
            // Merge into current coalesced buffer:
            // Update record count in header (offset 26..28) and append body records
            let existing_count =
                u16::from_le_bytes([self.coalesced_buf[26], self.coalesced_buf[27]]);
            let new_count = u16::from_le_bytes([block[26], block[27]]);
            let total_count = existing_count.saturating_add(new_count);

            self.coalesced_buf[26..28].copy_from_slice(&total_count.to_le_bytes());
            self.coalesced_buf
                .extend_from_slice(&block[TLV_HEADER_BYTES..]);
            self.last_received_mono_ns = received_at_ns;
            self.stats.coalesced_merges = self.stats.coalesced_merges.saturating_add(1);
        } else {
            // New capture timestamp: flush previously accumulated capture and start new buffer
            self.flush();
            self.current_capture_mono_ns = Some(capture_mono_ns);
            self.coalesced_buf.clear();
            self.coalesced_buf.extend_from_slice(block);
            self.last_received_mono_ns = received_at_ns;
            self.stats.deltas_received = self.stats.deltas_received.saturating_add(1);
        }
    }

    /// Flushes any pending coalesced capture block to the [`UiHub`].
    pub fn flush(&mut self) {
        if !self.coalesced_buf.is_empty() {
            let block: Arc<[u8]> = Arc::from(std::mem::take(&mut self.coalesced_buf));
            self.hub.push_block_with_gap(block, false, self.gap_pending);
            self.gap_pending = false;
            self.current_capture_mono_ns = None;
            self.stats.blocks_flushed = self.stats.blocks_flushed.saturating_add(1);
        }
    }

    /// Periodic 4 Hz tick: flushes any pending capture, evaluates stream age, and runs alerts.
    pub fn tick(
        &mut self,
        now_mono_ns: u64,
        engineer_relay_ns: Option<i64>,
        engineer_uncertainty_ns: Option<u64>,
    ) -> EngineerTickOutput {
        self.flush();

        let StreamAgeReport {
            stream_age_s,
            health,
            uncertainty_s: _,
            alerts,
        } = self.age_tracker.tick(
            now_mono_ns,
            engineer_relay_ns,
            engineer_uncertainty_ns,
            self.alert_engine.as_mut(),
        );

        let active_alerts = self.alert_engine.as_ref().map_or(0, |ae| ae.active().len());

        let status = EngineerStatusSnapshot {
            identity: self.active_identity.clone(),
            stream_health: health,
            stream_age_s,
            driver_away: self.driver_away(),
            ended_reason: self.ended_reason,
            incompatible_schema: self.incompatible_schema,
            active_alerts,
        };
        self.last_status = Some(status.clone());

        EngineerTickOutput { status, alerts }
    }
}

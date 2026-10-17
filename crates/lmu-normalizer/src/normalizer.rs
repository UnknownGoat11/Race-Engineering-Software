//! The stateful normalizer: one [`Normalizer`] per LMU connection, fed every reader snapshot in order.
//!
//! Per snapshot it:
//! 1. rebuilds the typed [`ScoringState`] when the scoring block changed (withdrawn as SUSPECT if the rows
//!    are inconsistent: `standings_withdrawn`);
//! 2. updates the [`SessionIdentity`] and emits `SessionChanged` / `TrackChanged` / `VehicleChanged`; any
//!    such change resets the session-scoped state (detectors, rate guards, capabilities, notices) **and the
//!    ET time base**, so the first sample of the new context is a first sample, not an ET regression;
//! 3. classifies the player telemetry ET: a frame whose `mElapsedTime` equals the previous one (same
//!    player `mID`) is a **duplicate and produces no physics sample**; a regression within the same
//!    identity is a session restart (reset + `SessionChanged{from: T, to: T}`, emitted once the scoring
//!    block has refreshed and only if that refresh did not itself report a session/track change), except a
//!    **session-end rewind** (session over, out of realtime, ET rewound to at or after `mEndET`,
//!    `discontinuity::is_session_end_rewind`): the same reset, but no restart event; a non-finite ET drops
//!    the sample;
//! 4. maps a new sample into a [`TelemetryFrame`], validates every value against the registry
//!    (`telemetry_model::validate`: finiteness, range, rate) and updates the capability map.

use std::collections::BTreeMap;

use lmu_reader::{
    RawGeneric, RawScoringInfo, RawSnapshot, RawTelemetry, RawVehicleScoring, ReaderStats,
    TelemetryHeader,
};
use serde::{Deserialize, Serialize};
use telemetry_model::enums::{CarDiscontinuity, SessionType, VehicleClass};
use telemetry_model::events::{EventRecord, EventTime, Severity};
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::registry;
use telemetry_model::scoring::{CapabilityMap, ScoringState, SessionIdentity, VehicleCapabilities};
use telemetry_model::validate::{EtStep, FrameValidator, Rejection, classify_et};

/// Consecutive published frames a channel must be rejected by validation before a
/// [`NoticeCode::ValueRejectedSustained`] diagnostic is raised (≈ 1 s at 100 Hz; A-LMU-049).
pub const SUSTAINED_REJECTION_FRAMES: u32 = 100;
/// Fastest plausible motion (m/s) if the registry ever lost the position rate bound (it has one: 200 m/s).
const FALLBACK_MAX_MOTION_MPS: f64 = 200.0;
/// Calculated channels the normalizer writes outside [crate::mapping::map_frame] (they need the
/// step-level state: ET classification and the discontinuity detector). Every published frame carries them.
pub const DERIVED_PER_STEP: &[telemetry_model::ChannelId] =
    &[telemetry_model::ch::SRC_DISCONTINUITY];
use telemetry_model::{ChannelId, Quality, ch};

use crate::capability::{self, VehicleCapabilityTracker};
use crate::detect::{
    BrakeTempUnit, BrakeTempUnitDecision, FillState, FractionScale, UnitDecisionState,
};
use crate::discontinuity::{
    CarSample, Discontinuity, DiscontinuityDetector, TELEPORT_ET_STEP_BACK_MAX_S,
    is_session_end_rewind, pit_service_channels,
};
use crate::identity::IdentityTracker;
use crate::mapping::{
    self, FILL_CHECKED_CHANNELS, MapInput, MapState, SessionDetectors, decode_current_sector,
    wheel_id,
};
use crate::notice::{DiagnosticNotice, NoticeCode, NoticeLog, push_logged};
use crate::standings::{self, PlayerRowIssue};
use crate::writer::FrameWriter;
use telemetry_model::registry::Wheel;

/// What happened to the player telemetry of one snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetryDisposition {
    /// A new physics sample was published.
    NewSample,
    /// A new sample after the ET went backwards (session restart; state was reset first).
    NewSampleAfterRegression,
    /// Same `mElapsedTime` as the previous sample: no new physics sample. Also used for a sample inside a
    /// garage-teleport ET step-back (A-LMU-031 kind (b), counted in
    /// [`NormalizerStats::teleport_et_step_backs`]): it is dropped so that the published ET stays monotonic.
    Duplicate,
    /// No verified player telemetry in this snapshot.
    NoPlayer,
    /// The player ET is NaN or infinite: the sample was dropped.
    NonFiniteEt,
}

/// Everything one snapshot produced.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedOutput {
    /// The new physics frame (only for [`TelemetryDisposition::NewSample`] and
    /// [`TelemetryDisposition::NewSampleAfterRegression`]).
    pub frame: Option<TelemetryFrame>,
    pub disposition: TelemetryDisposition,
    /// The new standings, when the scoring block changed and is consistent.
    pub scoring: Option<ScoringState>,
    /// `true` when this step's scoring update was inconsistent and the standings were withdrawn
    /// ([`Normalizer::scoring`] is now `None`); distinguishes "withdrawn" from "unchanged" (`scoring: None`).
    pub standings_withdrawn: bool,
    /// The session identity, when it changed.
    pub identity: Option<SessionIdentity>,
    /// Session/track/vehicle changes.
    pub events: Vec<EventRecord>,
    /// Diagnostics raised by this snapshot.
    pub notices: Vec<DiagnosticNotice>,
    /// Samples the registry validation rejected in `frame` (now `Invalid`), with the reason.
    pub rejections: Vec<(ChannelId, Rejection)>,
    /// The vehicle capabilities, when they changed.
    pub vehicle_capabilities: Option<VehicleCapabilities>,
    /// `true` when a status or note in [`Normalizer::capability_map`] changed.
    pub capabilities_changed: bool,
}

impl NormalizedOutput {
    fn new(disposition: TelemetryDisposition) -> Self {
        Self {
            frame: None,
            disposition,
            scoring: None,
            standings_withdrawn: false,
            identity: None,
            events: Vec::new(),
            notices: Vec::new(),
            rejections: Vec::new(),
            vehicle_capabilities: None,
            capabilities_changed: false,
        }
    }
}

/// Counters for diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizerStats {
    pub snapshots: u64,
    pub frames_published: u64,
    pub duplicate_frames: u64,
    pub et_regressions: u64,
    pub non_finite_et: u64,
    pub no_player_snapshots: u64,
    /// Samples published as `Invalid` (failed conversion or validation).
    pub invalid_samples: u64,
    /// Samples the frame rejected (mapping bugs; expected 0).
    pub mapping_errors: u64,
    /// Scoring updates processed (one per changed scoring block, not per snapshot).
    pub scoring_updates: u64,
    /// Scoring updates dropped as inconsistent.
    pub scoring_rejected: u64,
    pub session_resets: u64,
    /// Garage teleports recognised (rate history reset instead of rejecting, A-LMU-067).
    #[serde(default)]
    pub teleports: u64,
    /// Realtime entries that reset the rate history (A-LMU-067).
    #[serde(default)]
    pub realtime_entries: u64,
    /// Published samples taken while the car stood in the pit area (service channels' rate history reset).
    #[serde(default)]
    pub pit_service_samples: u64,
    /// ET regressions recognised as a session-end rewind (session over, out of realtime, ET rewound to the
    /// session end: time base reset, no restart event; A-LMU-031). Included in `et_regressions`.
    #[serde(default)]
    pub session_end_rewinds: u64,
    /// ET regressions recognised as a garage-teleport step-back (at most
    /// [`TELEPORT_ET_STEP_BACK_MAX_S`], same player, car moved into the pit area at rest; A-LMU-031 kind (b)):
    /// sample dropped, rate history reset, no session reset and no restart event. Included in
    /// `et_regressions`.
    #[serde(default)]
    pub teleport_et_step_backs: u64,
}

/// The raw inputs of one step, independent of how they were obtained (live reader, capture replay, test).
#[derive(Debug, Clone, Copy)]
pub struct NormalizerInput<'a> {
    /// Reader monotonic capture time (ns).
    pub capture_mono_ns: u64,
    pub generic: &'a RawGeneric,
    pub scoring_info: &'a RawScoringInfo,
    /// Scoring rows `0..min(mNumVehicles, 104)`.
    pub vehicles: &'a [RawVehicleScoring],
    /// The scoring block changed since the previous step.
    pub scoring_updated: bool,
    pub telemetry_header: &'a TelemetryHeader,
    /// The verified player telemetry slot, if any.
    pub player: Option<&'a RawTelemetry>,
    /// `generic.gameVersion` is a verified version (layout confirmed at runtime).
    pub version_verified: bool,
}

impl<'a> NormalizerInput<'a> {
    /// The inputs of a reader snapshot. `snapshot.player` is only ever the `mID`-verified player.
    pub fn from_snapshot(snapshot: &'a RawSnapshot) -> Self {
        Self {
            capture_mono_ns: snapshot.capture_mono_ns,
            generic: &snapshot.generic,
            scoring_info: &snapshot.scoring.info,
            vehicles: &snapshot.scoring.vehicles,
            scoring_updated: snapshot.scoring_updated,
            telemetry_header: &snapshot.telemetry_header,
            player: snapshot.player.as_ref().map(|p| &p.telemetry),
            version_verified: snapshot.version_verified,
        }
    }
}

/// Capability notes for channels whose meaning rests on an unverified assumption (shown in the UI).
const STATIC_NOTES: &[(ChannelId, &str)] = &[
    (
        ch::G_LAT,
        "+ = towards the car's left (ISO 8855); sign verified at Gate 1 (A-LMU-024)",
    ),
    (
        ch::G_LONG,
        "+ = accelerating, − = braking; sign verified at Gate 1 (A-LMU-024)",
    ),
    (
        ch::G_VERT,
        "kinematic, gravity excluded (≈ 0 g at rest); verified at Gate 1 (A-LMU-032)",
    ),
    (
        ch::YAW_RATE,
        "+ = yawing left (ISO 8855); sign verified at Gate 1 (A-LMU-024)",
    ),
    (
        ch::PITCH_RATE,
        "+ = nose down (ISO 8855); sign unverified (A-LMU-024)",
    ),
    (
        ch::ROLL_RATE,
        "+ = rolling right (ISO 8855); sign unverified (A-LMU-024)",
    ),
    (
        ch::STEERING_ANGLE,
        "assumes the range is lock-to-lock degrees (A-LMU-037)",
    ),
    (ch::ENGINE_POWER, "assumes mEngineTorque is N·m (A-LMU-053)"),
    (
        ch::HEADING,
        "zero reference and direction unverified (A-LMU-025)",
    ),
];

/// Per-wheel notes (FL id; applied to all four wheels).
const STATIC_WHEEL_NOTES: &[(ChannelId, &str)] = &[
    (
        ch::TYRE_TEMP_INNER_FL,
        "inner/outer side mapping verified at Gate 1 (stint1, LMGT3; A-LMU-016)",
    ),
    (
        ch::TYRE_TEMP_OUTER_FL,
        "inner/outer side mapping verified at Gate 1 (stint1, LMGT3; A-LMU-016)",
    ),
    (
        ch::TYRE_TEMP_SPREAD_FL,
        "inner/outer side mapping verified at Gate 1 (stint1, LMGT3; A-LMU-016)",
    ),
    (
        ch::WHEEL_SPEED_FL,
        "estimated from the static radius (A-LMU-039)",
    ),
];

/// The LMU → canonical normalizer. Deterministic: the same inputs in the same order produce the same
/// outputs. No global state; all state is session scoped and reset on session changes.
#[derive(Debug)]
pub struct Normalizer {
    sequence: u64,
    last_et: Option<f64>,
    last_player_id: Option<i32>,
    validator: FrameValidator,
    det: SessionDetectors,
    notice_log: NoticeLog,
    vehicle_caps: VehicleCapabilityTracker,
    capability_map: CapabilityMap,
    rates_hz: (Option<f64>, Option<f64>),
    identity: IdentityTracker,
    scoring: Option<ScoringState>,
    /// At least one scoring block was processed (consistent or not).
    scoring_seen: bool,
    /// An ET regression whose restart event waits for the next scoring refresh (see module docs).
    pending_restart: Option<(EventTime, Option<u32>)>,
    /// Consecutive-frame validation rejections per channel (bounded by the registry size).
    rejection_streaks: BTreeMap<ChannelId, u32>,
    /// Garage teleports, realtime entries and pit services (A-LMU-067).
    discontinuity: DiscontinuityDetector,
    /// Channels whose rate history a pit service resets.
    service_channels: Vec<ChannelId>,
    /// Fastest plausible motion (m/s): the registry's position rate bound.
    max_motion_mps: f64,
    /// A garage-teleport ET step-back is in progress: further samples still behind `last_et` (within
    /// [`TELEPORT_ET_STEP_BACK_MAX_S`]) belong to it. Cleared by the next advancing sample.
    teleport_step_back_open: bool,
    /// Strongest discontinuity seen since the last published frame that the next frame must report: a
    /// dropped teleport step-back sample or a session-end rewind (source.discontinuity).
    pending_discontinuity: CarDiscontinuity,
    stats: NormalizerStats,
}

impl Default for Normalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Normalizer {
    /// A normalizer with no history.
    pub fn new() -> Self {
        let mut n = Self {
            sequence: 0,
            last_et: None,
            last_player_id: None,
            validator: FrameValidator::new(),
            det: SessionDetectors::default(),
            notice_log: NoticeLog::default(),
            vehicle_caps: VehicleCapabilityTracker::default(),
            capability_map: CapabilityMap::from_registry(),
            rates_hz: (None, None),
            identity: IdentityTracker::default(),
            scoring: None,
            scoring_seen: false,
            pending_restart: None,
            rejection_streaks: BTreeMap::new(),
            discontinuity: DiscontinuityDetector::default(),
            service_channels: pit_service_channels(),
            max_motion_mps: registry::by_id(ch::POS_X)
                .and_then(|d| d.max_rate_per_s)
                .unwrap_or(FALLBACK_MAX_MOTION_MPS),
            teleport_step_back_open: false,
            pending_discontinuity: CarDiscontinuity::None,
            stats: NormalizerStats::default(),
        };
        n.capability_map = n.fresh_capability_map();
        n
    }

    /// Per-channel capability for the current car and session.
    pub fn capability_map(&self) -> &CapabilityMap {
        &self.capability_map
    }

    /// Capabilities of the current vehicle (decided from data).
    pub fn vehicle_capabilities(&self) -> VehicleCapabilities {
        self.vehicle_caps.capabilities()
    }

    /// Current (last known) session identity.
    pub fn identity(&self) -> &SessionIdentity {
        self.identity.current()
    }

    /// Latest consistent standings.
    pub fn scoring(&self) -> Option<&ScoringState> {
        self.scoring.as_ref()
    }

    /// The brake temperature unit currently applied and its confidence (A-LMU-015).
    pub fn brake_temp_unit(&self) -> BrakeTempUnitDecision {
        self.det.brake_unit.decision()
    }

    /// Detected scale of `mVirtualEnergy`.
    pub fn virtual_energy_scale(&self) -> FractionScale {
        self.det.virtual_energy.scale()
    }

    /// Detected scale of `mSoC`.
    pub fn soc_scale(&self) -> FractionScale {
        self.det.soc.scale()
    }

    /// Counters.
    pub fn stats(&self) -> NormalizerStats {
        self.stats
    }

    /// Records measured native rates (Hz) in the capability map; `None` leaves a rate unmeasured.
    pub fn set_native_rates(&mut self, telemetry_hz: Option<f64>, scoring_hz: Option<f64>) {
        self.rates_hz = (telemetry_hz, scoring_hz);
        capability::apply_native_rates(&mut self.capability_map, telemetry_hz, scoring_hz);
    }

    /// Records the rates the reader measured (`ReaderStats::telemetry_rate` / `scoring_rate`).
    pub fn update_rates_from_stats(&mut self, stats: &ReaderStats) {
        self.set_native_rates(stats.telemetry_rate.rate_hz(), stats.scoring_rate.rate_hz());
    }

    /// Normalizes one reader snapshot.
    pub fn process(&mut self, snapshot: &RawSnapshot) -> NormalizedOutput {
        self.process_input(&NormalizerInput::from_snapshot(snapshot))
    }

    /// Normalizes one step given its raw inputs.
    pub fn process_input(&mut self, input: &NormalizerInput<'_>) -> NormalizedOutput {
        self.stats.snapshots = self.stats.snapshots.saturating_add(1);
        let mut out = NormalizedOutput::new(TelemetryDisposition::NoPlayer);
        if !input.version_verified {
            self.notice_log.raise(
                DiagnosticNotice::new(
                    NoticeCode::GameVersionUnverified,
                    Severity::Warning,
                    Some(ch::GAME_VERSION),
                    "LMU game version not in the verified list: layout unconfirmed for this build",
                )
                .with_raw(f64::from(input.generic.game_version)),
                &mut out.notices,
            );
        }

        self.update_scoring(input, &mut out);
        let event_time = EventTime {
            source_et_s: input
                .player
                .map(|t| t.elapsed_time)
                .or(Some(input.scoring_info.current_et))
                .filter(|et| et.is_finite()),
            mono_ns: input.capture_mono_ns,
        };
        let lap = input.player.and_then(|t| u32::try_from(t.lap_number).ok());
        self.update_identity(input, event_time, lap, &mut out);
        // Every snapshot (with or without player telemetry), after a possible reset: the realtime entry
        // opens the cold-brake evidence window (A-LMU-015).
        self.det
            .brake_unit
            .note_realtime(input.scoring_info.in_realtime);
        self.discontinuity
            .note_realtime(input.scoring_info.in_realtime);

        let Some(telemetry) = input.player else {
            self.stats.no_player_snapshots = self.stats.no_player_snapshots.saturating_add(1);
            return out;
        };
        let et = telemetry.elapsed_time;
        let same_player = self.last_player_id == Some(telemetry.id);
        match classify_et(self.last_et, et) {
            EtStep::NonFinite => {
                self.stats.non_finite_et = self.stats.non_finite_et.saturating_add(1);
                self.notice_log.raise(
                    DiagnosticNotice::new(
                        NoticeCode::NonFiniteEt,
                        Severity::Warning,
                        Some(ch::ELAPSED_TIME),
                        "player mElapsedTime is not finite: sample dropped",
                    )
                    .with_raw(et),
                    &mut out.notices,
                );
                out.disposition = TelemetryDisposition::NonFiniteEt;
                return out;
            }
            EtStep::Unchanged if same_player => {
                self.stats.duplicate_frames = self.stats.duplicate_frames.saturating_add(1);
                out.disposition = TelemetryDisposition::Duplicate;
                return out;
            }
            EtStep::Regressed { by_s } => {
                self.stats.et_regressions = self.stats.et_regressions.saturating_add(1);
                if same_player && by_s <= TELEPORT_ET_STEP_BACK_MAX_S {
                    let row = standings::player_row(input.vehicles, telemetry.id).ok();
                    let sample = car_sample(telemetry, row);
                    let opening = if self.discontinuity.is_step_back_teleport(&sample) {
                        Some(StepBackOpening::WithJump)
                    } else if !self.teleport_step_back_open
                        && self.discontinuity.is_step_back_after_last_teleport(&sample)
                    {
                        Some(StepBackOpening::AfterJump)
                    } else {
                        None
                    };
                    if opening.is_some() || self.teleport_step_back_open {
                        self.handle_teleport_step_back(sample, by_s, opening, &mut out);
                        return out;
                    }
                }
                let info = input.scoring_info;
                if is_session_end_rewind(
                    info.game_phase,
                    info.in_realtime,
                    mapping::session_end_et(info.end_et),
                    et + by_s,
                    et,
                ) {
                    // The finished session is being closed, not restarted: the time base is void (reset),
                    // but no SessionChanged restart event is emitted.
                    self.stats.session_end_rewinds =
                        self.stats.session_end_rewinds.saturating_add(1);
                    push_logged(
                        DiagnosticNotice::new(
                            NoticeCode::EtRegression,
                            Severity::Notice,
                            Some(ch::ELAPSED_TIME),
                            format!(
                                "player ET went back by {by_s:.3} s to the session end after the session was over (session-end rewind): time base reset, not a restart"
                            ),
                        )
                        .with_raw(et)
                        .at(Some(et)),
                        &mut out.notices,
                    );
                    self.pending_restart = None;
                    self.reset_session_context();
                    // Reported on this sample (after the reset, which clears what the old context left).
                    self.pending_discontinuity = CarDiscontinuity::SessionEndRewind;
                    out.disposition = TelemetryDisposition::NewSampleAfterRegression;
                    self.publish_sample(input, telemetry, &mut out);
                    return out;
                }
                push_logged(
                    DiagnosticNotice::new(
                        NoticeCode::EtRegression,
                        Severity::Notice,
                        Some(ch::ELAPSED_TIME),
                        format!("player ET went back by {by_s:.3} s: session restart, state reset"),
                    )
                    .with_raw(et)
                    .at(Some(et)),
                    &mut out.notices,
                );
                // Telemetry can restart before the scoring block reports a new session type: the restart
                // event waits for a scoring refresh so that a real session change is reported once.
                if input.scoring_updated {
                    if let Some(event) = self.identity.restart_event() {
                        out.events.push(EventRecord::new(event_time, lap, event));
                    }
                } else {
                    self.pending_restart = Some((event_time, lap));
                }
                self.reset_session_context();
                out.disposition = TelemetryDisposition::NewSampleAfterRegression;
            }
            EtStep::First | EtStep::Advanced { .. } | EtStep::Unchanged => {
                self.teleport_step_back_open = false;
                if !same_player {
                    // Rates across two different cars are meaningless.
                    self.validator.reset();
                }
                out.disposition = TelemetryDisposition::NewSample;
            }
        }
        self.publish_sample(input, telemetry, &mut out);
        out
    }

    fn update_scoring(&mut self, input: &NormalizerInput<'_>, out: &mut NormalizedOutput) {
        // Only a changed scoring block is rebuilt (plus the very first one); a withdrawn state is not
        // rebuilt at telemetry rate.
        if !(input.scoring_updated || !self.scoring_seen) {
            return;
        }
        self.scoring_seen = true;
        self.stats.scoring_updates = self.stats.scoring_updates.saturating_add(1);
        let state = standings::scoring_state(
            input.scoring_info,
            input.vehicles,
            &mut self.det.time_of_day,
        );
        if state.is_consistent() {
            self.scoring = Some(state.clone());
            out.scoring = Some(state);
        } else {
            self.stats.scoring_rejected = self.stats.scoring_rejected.saturating_add(1);
            self.scoring = None;
            out.standings_withdrawn = true;
            self.notice_log.raise(
                DiagnosticNotice::new(
                    NoticeCode::StandingsInconsistent,
                    Severity::Warning,
                    None,
                    "scoring rows have duplicate mIDs or several player rows: standings dropped (SUSPECT)",
                ),
                &mut out.notices,
            );
        }
    }

    fn update_identity(
        &mut self,
        input: &NormalizerInput<'_>,
        at: EventTime,
        lap: Option<u32>,
        out: &mut NormalizedOutput,
    ) {
        let info = input.scoring_info;
        let player_row = input
            .player
            .and_then(|t| standings::player_row(input.vehicles, t.id).ok());
        let candidate = SessionIdentity {
            vehicle_name: input.player.and_then(|t| standings::text(&t.vehicle_name)),
            vehicle_model: input.player.and_then(|t| standings::text(&t.vehicle_model)),
            vehicle_class: input
                .player
                .map(|t| VehicleClass::from_raw(t.vehicle_class)),
            class_name: player_row.and_then(|r| standings::text(&r.vehicle_class)),
            track_name: standings::text(&info.track_name),
            // Only a session with a track loaded has a meaningful type (mSession reads 0 in menus too).
            session_type: standings::text(&info.track_name)
                .map(|_| SessionType::from_lmu(info.session)),
            game_version: (input.generic.game_version > 0).then_some(input.generic.game_version),
        };
        let before = self.identity.current().clone();
        let change = self.identity.observe(&candidate);
        if change.any() {
            self.reset_session_context();
        }
        if change.session_changed || change.track_changed {
            // The session change is the boundary: no extra restart event for the same transition.
            self.pending_restart = None;
        } else if input.scoring_updated
            && let Some((restart_at, restart_lap)) = self.pending_restart.take()
            && let Some(event) = self.identity.restart_event()
        {
            out.events
                .push(EventRecord::new(restart_at, restart_lap, event));
        }
        for event in change.events {
            out.events.push(EventRecord::new(at, lap, event));
        }
        if self.identity.current() != &before {
            out.identity = Some(self.identity.current().clone());
        }
    }

    /// Resets everything that is scoped to one session / vehicle context, including the ET time base (the
    /// next sample is a first sample, not a regression against the previous context).
    fn reset_session_context(&mut self) {
        self.stats.session_resets = self.stats.session_resets.saturating_add(1);
        self.last_et = None;
        self.last_player_id = None;
        self.teleport_step_back_open = false;
        self.pending_discontinuity = CarDiscontinuity::None;
        self.rejection_streaks.clear();
        self.discontinuity = DiscontinuityDetector::default();
        self.det = SessionDetectors::default();
        self.notice_log.reset();
        self.validator.reset();
        self.vehicle_caps.reset();
        self.capability_map = self.fresh_capability_map();
    }

    /// Registry defaults, the static notes and the last measured rates.
    fn fresh_capability_map(&self) -> CapabilityMap {
        let mut map = CapabilityMap::from_registry();
        for &(id, note) in STATIC_NOTES {
            capability::set_note(&mut map, id, Some(note));
        }
        for &(fl, note) in STATIC_WHEEL_NOTES {
            for wheel in Wheel::ALL {
                capability::set_note(&mut map, wheel_id(fl, wheel), Some(note));
            }
        }
        let (telemetry_hz, scoring_hz) = self.rates_hz;
        capability::apply_native_rates(&mut map, telemetry_hz, scoring_hz);
        map
    }

    fn publish_sample(
        &mut self,
        input: &NormalizerInput<'_>,
        telemetry: &RawTelemetry,
        out: &mut NormalizedOutput,
    ) {
        // Unknown player row: counted as outside the garage (the decision is never deferred for ever).
        let in_garage_stall = standings::player_row(input.vehicles, telemetry.id)
            .is_ok_and(|row| row.in_garage_stall);
        if self.vehicle_caps.observe_at(telemetry, in_garage_stall) {
            out.vehicle_capabilities = Some(self.vehicle_caps.capabilities());
        }
        let player_row = match standings::player_row(input.vehicles, telemetry.id) {
            Ok(row) => Some(row),
            Err(issue) => {
                let reason = match issue {
                    PlayerRowIssue::NoPlayerRow => "no scoring row is flagged mIsPlayer",
                    PlayerRowIssue::SeveralPlayerRows => {
                        "several scoring rows are flagged mIsPlayer"
                    }
                    PlayerRowIssue::IdMismatch => "the mIsPlayer row has another mID",
                };
                self.notice_log.raise(
                    DiagnosticNotice::new(
                        NoticeCode::PlayerScoringRowMissing,
                        Severity::Notice,
                        None,
                        format!("player scoring channels unavailable: {reason}"),
                    ),
                    &mut out.notices,
                );
                None
            }
        };
        let class_place = self
            .scoring
            .as_ref()
            .and_then(|s| s.by_slot(telemetry.id))
            .filter(|row| row.is_player)
            .and_then(|row| row.class_place);

        let et = telemetry.elapsed_time;
        self.sequence = self.sequence.saturating_add(1);
        let mut frame = TelemetryFrame::new(self.sequence, input.capture_mono_ns, Some(et));
        let mut writer = FrameWriter::new(&mut frame);
        let map_input = MapInput {
            telemetry,
            header: input.telemetry_header,
            generic: input.generic,
            info: input.scoring_info,
            player_row,
            class_place,
            caps: self.vehicle_caps.capabilities(),
        };
        let mut state = MapState {
            det: &mut self.det,
            notice_log: &mut self.notice_log,
            notices: &mut out.notices,
        };
        mapping::map_frame(&map_input, &mut state, &mut writer);
        let errors = writer.into_errors();
        if !errors.is_empty() {
            self.stats.mapping_errors = self
                .stats
                .mapping_errors
                .saturating_add(u64::try_from(errors.len()).unwrap_or(u64::MAX));
            for (id, err) in errors {
                self.notice_log.raise(
                    DiagnosticNotice::new(
                        NoticeCode::MappingError,
                        Severity::Critical,
                        Some(id),
                        format!("internal mapping error: {err}"),
                    ),
                    &mut out.notices,
                );
            }
        }

        // Known discontinuities reset the rate history they void, before validation (A-LMU-067), and are
        // published in source.discontinuity together with what the dropped samples left pending.
        let seen = self.apply_discontinuity(telemetry, player_row);
        let reported = std::mem::replace(&mut self.pending_discontinuity, CarDiscontinuity::None)
            .strongest(seen);
        self.write_discontinuity(&mut frame, reported);
        // Registry validation of every value: finiteness, plausible range, plausible rate of change.
        let validation = self.validator.validate(&mut frame);
        out.rejections = validation.rejected;
        self.track_rejections(&out.rejections, et, &mut out.notices);
        let invalid = frame
            .iter()
            .filter(|(_, s)| s.quality == Quality::Invalid)
            .count();
        self.stats.invalid_samples = self
            .stats
            .invalid_samples
            .saturating_add(u64::try_from(invalid).unwrap_or(u64::MAX));
        self.stats.frames_published = self.stats.frames_published.saturating_add(1);
        self.write_source_counters(&mut frame);

        out.capabilities_changed |= capability::update_from_frame(&mut self.capability_map, &frame);
        out.capabilities_changed |= self.apply_dynamic_notes();

        self.last_et = Some(et);
        self.last_player_id = Some(telemetry.id);
        out.frame = Some(frame);
    }

    /// A garage teleport whose ET stepped back by `by_s` (A-LMU-031 kind (b)): the same session goes on, so
    /// nothing session-scoped is reset and no restart event is emitted. The sample is dropped (the published
    /// ET stays monotonic, `last_et` keeps the larger ET) and the rate history is void as for any teleport.
    /// `opening` says how a new step-back was recognised (`None`: continuing an open one).
    fn handle_teleport_step_back(
        &mut self,
        sample: CarSample,
        by_s: f64,
        opening: Option<StepBackOpening>,
        out: &mut NormalizedOutput,
    ) {
        self.stats.teleport_et_step_backs = self.stats.teleport_et_step_backs.saturating_add(1);
        // The sample is dropped: the next published frame reports the step-back.
        self.pending_discontinuity = self
            .pending_discontinuity
            .strongest(CarDiscontinuity::TeleportStepBack);
        if let Some(how) = opening {
            self.teleport_step_back_open = true;
            if how == StepBackOpening::WithJump {
                // The jump is on this (dropped) sample: count it here. After a jump it was counted when
                // the teleport sample was published.
                self.stats.teleports = self.stats.teleports.saturating_add(1);
            }
            self.discontinuity.accept_step_back(sample);
            push_logged(
                DiagnosticNotice::new(
                    NoticeCode::EtRegression,
                    Severity::Notice,
                    Some(ch::ELAPSED_TIME),
                    format!(
                        "player ET went back by {by_s:.3} s at a garage teleport (A-LMU-031): sample dropped, rate history reset, not a restart"
                    ),
                )
                .with_raw(sample.et_s)
                .at(Some(sample.et_s)),
                &mut out.notices,
            );
        }
        self.validator.reset();
        out.disposition = TelemetryDisposition::Duplicate;
    }

    /// Recognises a garage teleport, a realtime entry or a pit service for this sample and resets the rate
    /// history of the channels it voids: everything for a teleport or a realtime entry, the fuel, energy and
    /// tyre channels for a service. Range checks still apply; an uncorroborated jump is still rejected.
    /// Returns what source.discontinuity reports for this sample (a pit service is not a discontinuity of
    /// the car's position or time base).
    fn apply_discontinuity(
        &mut self,
        t: &RawTelemetry,
        row: Option<&RawVehicleScoring>,
    ) -> CarDiscontinuity {
        let sample = car_sample(t, row);
        let seen = self.discontinuity.observe(sample, self.max_motion_mps);
        match seen {
            Some(Discontinuity::Teleport) => {
                self.stats.teleports = self.stats.teleports.saturating_add(1);
                tracing::debug!(
                    component = "lmu-normalizer",
                    et = t.elapsed_time,
                    "garage teleport: rate history reset"
                );
                self.validator.reset();
            }
            Some(Discontinuity::RealtimeEntry) => {
                self.stats.realtime_entries = self.stats.realtime_entries.saturating_add(1);
                self.validator.reset();
            }
            Some(Discontinuity::PitService) => {
                self.stats.pit_service_samples = self.stats.pit_service_samples.saturating_add(1);
                self.validator.reset_channels(&self.service_channels);
            }
            None => {}
        }
        match seen {
            Some(Discontinuity::Teleport) => CarDiscontinuity::Teleport,
            Some(Discontinuity::RealtimeEntry) => CarDiscontinuity::RealtimeEntry,
            Some(Discontinuity::PitService) | None => CarDiscontinuity::None,
        }
    }

    /// Writes source.discontinuity (formula discontinuity_teleport_rewind_entry) into a published frame.
    fn write_discontinuity(&mut self, frame: &mut TelemetryFrame, kind: CarDiscontinuity) {
        let mut w = FrameWriter::new(frame);
        w.int(ch::SRC_DISCONTINUITY, i64::from(kind.code()));
        let errors = w.into_errors();
        self.stats.mapping_errors = self
            .stats
            .mapping_errors
            .saturating_add(u64::try_from(errors.len()).unwrap_or(u64::MAX));
        for (id, err) in errors {
            tracing::error!(component = "lmu-normalizer", channel = id.0, error = %err, "discontinuity channel");
        }
    }

    /// Counts consecutive rejections per channel and raises a deduplicated diagnostic when a channel stays
    /// rejected for [`SUSTAINED_REJECTION_FRAMES`] frames (A-LMU-049: e.g. a unit mix-up).
    fn track_rejections(
        &mut self,
        rejections: &[(ChannelId, Rejection)],
        et: f64,
        notices: &mut Vec<DiagnosticNotice>,
    ) {
        let mut next = BTreeMap::new();
        for (id, why) in rejections {
            let streak = self
                .rejection_streaks
                .get(id)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            next.insert(*id, streak);
            if streak == SUSTAINED_REJECTION_FRAMES {
                let mut notice = DiagnosticNotice::new(
                    NoticeCode::ValueRejectedSustained,
                    Severity::Warning,
                    Some(*id),
                    format!("rejected by validation for {streak} consecutive frames: {why}"),
                )
                .at(Some(et));
                if let Rejection::OutOfRange { value, .. } = why {
                    notice = notice.with_raw(*value);
                }
                self.notice_log.raise(notice, notices);
            }
        }
        // Channels not rejected in this frame restart from zero.
        self.rejection_streaks = next;
    }

    /// Source diagnostics owned by the normalizer (Application channels).
    fn write_source_counters(&self, frame: &mut TelemetryFrame) {
        let mut w = FrameWriter::new(frame);
        let as_i64 = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        w.int(ch::SRC_ET_REGRESSIONS, as_i64(self.stats.et_regressions));
        w.int(
            ch::SRC_DUPLICATE_FRAMES,
            as_i64(self.stats.duplicate_frames),
        );
        w.int(ch::SRC_INVALID_SAMPLES, as_i64(self.stats.invalid_samples));
        for (id, err) in w.into_errors() {
            tracing::error!(component = "lmu-normalizer", channel = id.0, error = %err, "source counter");
        }
    }

    /// Capability notes that follow the detectors; returns `true` if any changed.
    fn apply_dynamic_notes(&mut self) -> bool {
        let map = &mut self.capability_map;
        let mut changed = false;
        let decision = self.det.brake_unit.decision();
        let brake_note = match (decision.state, decision.unit) {
            (UnitDecisionState::Decided, BrakeTempUnit::Kelvin) => {
                "unit detected from cold-brake evidence: Kelvin (A-LMU-015)"
            }
            (UnitDecisionState::Decided, BrakeTempUnit::Celsius) => {
                "unit detected from cold-brake evidence: Celsius (A-LMU-015)"
            }
            (UnitDecisionState::Verified, BrakeTempUnit::Kelvin) => {
                "Kelvin, verified with real data for this LMU version (Gate 1, A-LMU-015)"
            }
            (UnitDecisionState::Verified, BrakeTempUnit::Celsius) => {
                "Celsius, verified with real data for this LMU version (A-LMU-015)"
            }
            (UnitDecisionState::Conflicting, _) => {
                "unit unverified: conflicting K/°C evidence, withheld for the session (A-LMU-015)"
            }
            (UnitDecisionState::Assumed, _) => {
                "unit unverified (K or °C): withheld until cold-brake evidence decides (A-LMU-015)"
            }
        };
        let pressure_note = if self.det.brake_pressure.unit_changed() {
            "values above 1.5 seen: unit changed (kPa?), mapping stopped (A-LMU-017)"
        } else {
            "0..1 fraction (driver input × balance), not a line pressure (A-LMU-017)"
        };
        for wheel in Wheel::ALL {
            changed |=
                capability::set_note(map, wheel_id(ch::BRAKE_TEMP_FL, wheel), Some(brake_note));
            changed |= capability::set_note(
                map,
                wheel_id(ch::BRAKE_PRESSURE_FL, wheel),
                Some(pressure_note),
            );
        }
        let scale_note = |scale: FractionScale, verified: bool| match scale {
            FractionScale::Undecided => {
                "scale undecided (0..1 or %): withheld until a value > 1.5 or two laps of changing values \
                 decide it (A-LMU-013/014)"
            }
            FractionScale::Fraction if verified => {
                "0..1, verified with real data for this LMU version (Gate 1, A-LMU-013)"
            }
            FractionScale::Fraction => {
                "0..1 confirmed by two laps of changing values in 0..1.5 (A-LMU-013/014)"
            }
            FractionScale::Percent => {
                "percent detected (values > 1.5), divided by 100 (A-LMU-013/014)"
            }
            FractionScale::Unrecognized => "unrecognized scale: published INVALID (A-LMU-013/014)",
            FractionScale::Conflicting => {
                "value contradicts the scale verified for this LMU version: withheld for the session \
                 (A-LMU-013/014)"
            }
        };
        let caps = self.vehicle_caps.capabilities();
        let ve_note = if caps.supports_virtual_energy == Some(false) {
            "car reports no virtual energy"
        } else {
            let ve = &self.det.virtual_energy;
            scale_note(ve.scale(), ve.is_verified())
        };
        changed |= capability::set_note(map, ch::VIRTUAL_ENERGY, Some(ve_note));
        let soc = &self.det.soc;
        changed |= capability::set_note(
            map,
            ch::BATTERY_SOC,
            Some(scale_note(soc.scale(), soc.is_verified())),
        );
        for &(id, _) in FILL_CHECKED_CHANNELS {
            let note = (self.det.fill_state(id) == FillState::NotFilled).then_some(
                "never changed from its first value while the car drove: not filled by LMU for this \
                 car/version, withheld (A-LMU-065)",
            );
            changed |= capability::set_note(map, id, note);
        }
        let hybrid_note = (caps.supports_hybrid == Some(false))
            .then_some("car reports no hybrid system (electric motor state unavailable)");
        for id in capability::HYBRID_GATED {
            if id != ch::BATTERY_SOC {
                changed |= capability::set_note(map, id, hybrid_note);
            }
        }
        let wear_note = caps.supports_tyre_wear.is_none().then_some(
            "no non-zero mWear yet (0 = not simulated or a new tyre, direction unverified): withheld \
             (A-LMU-040/057)",
        );
        for wheel in Wheel::ALL {
            changed |= capability::set_note(map, wheel_id(ch::TYRE_WEAR_FL, wheel), wear_note);
        }
        let tod_note = (!self.det.time_of_day.is_decided())
            .then_some("hours vs seconds undecided: withheld (A-LMU-036)");
        changed |= capability::set_note(map, ch::TIME_OF_DAY, tod_note);
        changed
    }
}

/// The discontinuity facts of one player telemetry sample.
/// How a new garage-teleport ET step-back was recognised (A-LMU-031 kind (b)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepBackOpening {
    /// The jump into the garage is on the step-back sample itself
    /// ([`DiscontinuityDetector::is_step_back_teleport`]).
    WithJump,
    /// The step-back follows the published teleport sample
    /// ([`DiscontinuityDetector::is_step_back_after_last_teleport`]; Gate 1 pitstop1).
    AfterJump,
}

fn car_sample(t: &RawTelemetry, row: Option<&RawVehicleScoring>) -> CarSample {
    let (_, pit_lane_bit) = decode_current_sector(t.current_sector);
    CarSample {
        et_s: t.elapsed_time,
        pos: [t.pos.x, t.pos.y, t.pos.z],
        speed_mps: crate::convention::speed_mps(t.local_vel),
        in_pit_area: pit_lane_bit || row.is_some_and(|r| r.in_pits || r.in_garage_stall),
    }
}

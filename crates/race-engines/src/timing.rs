//! Timing engine (spec §16–19, §65, §144): lap/sector state machine, lap history, bests, theoretical best,
//! reference laps and the distance-aligned live delta.
//!
//! # Lap transitions (spec §65)
//! Three independent signals are watched on every sample:
//! - the LMU lap counter (`lap.number`) incrementing by exactly one;
//! - a new LMU lap start time (`lap.start_et_s`) close to the sample time;
//! - the lap distance wrapping from the end of the lap to its start (with re-arm hysteresis).
//!
//! The *primary* signal decides that a lap ended: the counter when it is available, else the lap start
//! time, else the distance wrap. Every primary signal can fire at most once per lap (the counter only moves
//! forward by one, the lap start only changes at the line, the wrap must be re-armed past half a lap), so a
//! lap is never counted twice. The other signals are *evidence*: they may arrive up to
//! [`CONFIRMATION_WINDOW_S`] before or after the primary one (scoring-derived distance lags the telemetry
//! counter). A lap's time is trusted only with at least two pieces of evidence at both ends; untrusted laps
//! are kept in the history but never become bests or references. A counter jump (missed laps, rejoin) or an
//! elapsed-time regression abandons the lap in progress instead of completing it.
//!
//! # Sectors (spec §16)
//! The sector model is dynamic: either the source sector index (LMU reports 3, the count is learned from
//! the indices seen) or explicit lap-distance boundaries from the track configuration (any count up to
//! [`MAX_SECTORS`]).
//!
//! # Live delta (spec §17–18)
//! Every lap stores a `(distance, elapsed)` trace at [`TRACE_RESOLUTION_M`] resolution. The live delta is the
//! current lap's elapsed time minus the reference lap's elapsed time interpolated at the *same lap distance*
//! — never `current elapsed − best lap time`. References never mix tracks, classes, vehicles or sessions
//! (see [`CompatibilityKey`]).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use telemetry_model::{TelemetryFrame, ch};
use thiserror::Error;
use tracing::{debug, warn};

use crate::common::{
    CompatibilityKey, Metric, finite, formula, frame_bool, frame_f64, frame_int, lerp,
};

/// Tracing component name.
const COMPONENT: &str = "race_engines::timing";

/// Minimum distance advance (m) between two stored trace points.
pub const TRACE_RESOLUTION_M: f64 = 2.0;
/// A forward distance jump larger than this (m) between samples makes the lap trace unusable as a
/// reference (data gap, teleport).
pub const MAX_TRACE_GAP_M: f64 = 150.0;
/// Upper bound of points stored for one lap (13.6 km at 2 m is 6 800 points; this leaves a wide margin).
pub const MAX_TRACE_POINTS: usize = 50_000;
/// Minimum number of trace points for a lap to serve as a reference.
pub const MIN_TRACE_POINTS: usize = 10;
/// The previous sample must be beyond this fraction of the lap for a distance wrap.
pub const WRAP_HIGH_FRACTION: f64 = 0.85;
/// The current sample must be below this fraction of the lap for a distance wrap.
pub const WRAP_LOW_FRACTION: f64 = 0.15;
/// A wrap is re-armed only after the car passed this fraction of the lap (no double wrap at the line).
pub const WRAP_REARM_FRACTION: f64 = 0.5;
/// A new LMU lap start time must be within this many seconds of the sample time.
pub const LAP_START_ET_TOLERANCE_S: f64 = 1.0;
/// Signals within this window (s) of each other confirm the same line crossing.
pub const CONFIRMATION_WINDOW_S: f64 = 3.0;
/// Shortest plausible lap (s); no LMU circuit is faster. Shorter "laps" are never trusted.
pub const MIN_LAP_TIME_S: f64 = 10.0;
/// Longest plausible lap (s), matching the registry's lap-time range.
pub const MAX_LAP_TIME_S: f64 = 3_600.0;
/// An official LMU last-lap time is adopted when it differs from the computed time by at most this (s).
pub const OFFICIAL_LAP_TIME_TOLERANCE_S: f64 = 0.25;
/// An official last-lap time update is matched to the lap completed at most this long before (s).
pub const OFFICIAL_LAP_ADOPTION_WINDOW_S: f64 = 10.0;
/// Bound of the lap history (a 24-hour race at Le Mans is about 400 laps).
pub const MAX_LAP_HISTORY: usize = 2_000;
/// Bound of the stored lap traces (pinned references included).
pub const MAX_STORED_TRACES: usize = 16;
/// Upper bound of the dynamic sector count.
pub const MAX_SECTORS: usize = 64;
/// Minimum evidence (signals agreeing) for a trusted lap boundary.
pub const MIN_BOUNDARY_EVIDENCE: u8 = 2;

/// Errors of the timing engine's configuration and reference API.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum TimingError {
    /// Distance sector boundaries must be finite, positive and strictly increasing.
    #[error("invalid sector boundaries: {reason}")]
    InvalidSectorBoundaries { reason: &'static str },
    /// More sectors than [`MAX_SECTORS`].
    #[error("{count} sectors exceed the maximum of {MAX_SECTORS}")]
    TooManySectors { count: usize },
    /// The trace resolution must be finite and positive.
    #[error("invalid trace resolution {0} m")]
    InvalidTraceResolution(f64),
    /// The lap is not in the history.
    #[error("unknown lap {0:?}")]
    UnknownLap(LapId),
    /// The lap was recorded under an incompatible track / vehicle / class / session.
    #[error("lap {0:?} is not compatible with the current context")]
    IncompatibleLap(LapId),
    /// The lap has no complete distance trace.
    #[error("lap {0:?} has no complete distance trace")]
    NoTrace(LapId),
    /// An imported trace is malformed.
    #[error("invalid lap trace: {0}")]
    InvalidTrace(&'static str),
}

/// Engine-assigned lap identifier, unique within one engine instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LapId(pub u32);

/// Timing inputs of one telemetry sample. All times share LMU's elapsed-time base.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimingSample {
    /// LMU elapsed session time of the sample (s).
    pub et_s: f64,
    /// LMU lap counter (`mLapNumber`).
    pub lap_number: Option<i32>,
    /// LMU start time of the current lap (`mLapStartET`, s).
    pub lap_start_et_s: Option<f64>,
    /// Distance from the start/finish line along the lap (m).
    pub lap_dist_m: Option<f64>,
    /// Track length (m).
    pub track_length_m: Option<f64>,
    /// Zero-based sector index from the source (LMU: 0..=2).
    pub sector_index: Option<u8>,
    /// LMU lap invalidation flag (track limits).
    pub lap_invalidated: Option<bool>,
    /// Car is in the pit lane.
    pub in_pit_lane: Option<bool>,
    /// Official last lap time from scoring (s), adopted when it matches the computed time.
    pub official_last_lap_s: Option<f64>,
}

impl TimingSample {
    /// A sample with only its time known.
    pub fn new(et_s: f64) -> Self {
        Self {
            et_s,
            ..Self::default()
        }
    }

    /// Builds the sample from a canonical frame. The lap distance prefers the telemetry-rate lap progress
    /// times the track length over the scoring-rate lap distance. `None` when the sample time is unknown.
    pub fn from_frame(frame: &TelemetryFrame) -> Option<Self> {
        let et_s = finite(frame.source_et_s).or_else(|| frame_f64(frame, ch::ELAPSED_TIME))?;
        let track_length_m = frame_f64(frame, ch::TRACK_LENGTH).filter(|l| *l > 0.0);
        let progress_dist = frame_f64(frame, ch::LAP_PROGRESS)
            .zip(track_length_m)
            .map(|(p, l)| p * l);
        Some(Self {
            et_s,
            lap_number: frame_int(frame, ch::LAP_NUMBER).and_then(|v| i32::try_from(v).ok()),
            lap_start_et_s: frame_f64(frame, ch::LAP_START_TIME),
            lap_dist_m: progress_dist.or_else(|| frame_f64(frame, ch::LAP_DISTANCE)),
            track_length_m,
            sector_index: frame_int(frame, ch::SECTOR).and_then(|v| u8::try_from(v).ok()),
            lap_invalidated: frame_bool(frame, ch::LAP_INVALID),
            in_pit_lane: frame_bool(frame, ch::IN_PIT_LANE),
            official_last_lap_s: frame_f64(frame, ch::LAST_LAP_TIME),
        })
    }
}

/// How sectors are delimited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SectorModel {
    /// Sectors follow the source sector index; the count is learned from the indices seen.
    SourceIndex,
    /// Sector start distances (m from the line), strictly increasing and excluding 0; `n` boundaries give
    /// `n + 1` sectors.
    DistanceBoundaries(Vec<f64>),
}

impl SectorModel {
    /// Checks the boundaries of a distance model.
    pub fn validate(&self) -> Result<(), TimingError> {
        let Self::DistanceBoundaries(b) = self else {
            return Ok(());
        };
        if b.len() + 1 > MAX_SECTORS {
            return Err(TimingError::TooManySectors { count: b.len() + 1 });
        }
        if b.iter().any(|x| !x.is_finite() || *x <= 0.0) {
            return Err(TimingError::InvalidSectorBoundaries {
                reason: "boundaries must be finite and positive",
            });
        }
        if b.windows(2).any(|w| w[1] <= w[0]) {
            return Err(TimingError::InvalidSectorBoundaries {
                reason: "boundaries must be strictly increasing",
            });
        }
        Ok(())
    }
}

/// Timing engine configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimingConfig {
    /// Sector model.
    pub sector_model: SectorModel,
    /// Allow laps of other sessions (same track, vehicle and class) as bests and references — only on an
    /// explicit engineer request (spec §18).
    pub allow_cross_session_references: bool,
    /// Trace resolution (m).
    pub trace_resolution_m: f64,
}

impl Default for TimingConfig {
    fn default() -> Self {
        Self {
            sector_model: SectorModel::SourceIndex,
            allow_cross_session_references: false,
            trace_resolution_m: TRACE_RESOLUTION_M,
        }
    }
}

/// Which signals confirmed a lap boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionEvidence {
    /// The LMU lap counter incremented by one.
    pub lap_counter: bool,
    /// LMU reported a new lap start time.
    pub lap_start_time: bool,
    /// The lap distance wrapped.
    pub distance_wrap: bool,
}

impl TransitionEvidence {
    /// Number of agreeing signals.
    pub fn count(&self) -> u8 {
        u8::from(self.lap_counter) + u8::from(self.lap_start_time) + u8::from(self.distance_wrap)
    }
}

/// Where a lap boundary time comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoundarySource {
    /// LMU's own lap start time (`mLapStartET`).
    LmuLapStart,
    /// Interpolated line crossing between two samples (distance wrap).
    Interpolated,
    /// Time of the sample on which the counter moved (one sample interval of uncertainty).
    SampleTime,
}

/// Where a completed lap's time comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LapTimeSource {
    /// Official LMU last-lap time (scoring), matched to the computed time.
    Official,
    /// Difference of two LMU lap start times.
    LmuLapStart,
    /// At least one boundary interpolated or taken from a sample time.
    Interpolated,
    /// Imported from a recording.
    Imported,
}

/// Facts about a lap that decide its eligibility.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LapFlags {
    /// LMU invalidated the lap (track limits).
    pub invalidated: bool,
    /// The car entered the pit lane during the lap (in-lap).
    pub pit_in: bool,
    /// The lap started in the pit lane (out-lap).
    pub pit_out: bool,
    /// The engine did not observe the lap's start (joined mid-lap or after an abandoned lap).
    pub joined_mid_lap: bool,
    /// The distance trace has a gap.
    pub trace_gap: bool,
    /// A sector transition was skipped or out of order; sector times of this lap are not used.
    pub sector_timing_broken: bool,
    /// Evidence at the lap start.
    pub start_evidence: TransitionEvidence,
    /// Evidence at the lap end.
    pub end_evidence: TransitionEvidence,
    /// Imported from a recording.
    pub imported: bool,
}

/// One completed lap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapRecord {
    /// Engine lap id.
    pub id: LapId,
    /// LMU lap number of this lap (`None` if unknown).
    pub lap_number: Option<i32>,
    /// Context the lap was driven in.
    pub key: CompatibilityKey,
    /// Lap start (s, LMU elapsed time) if known.
    pub start_et_s: Option<f64>,
    /// Lap end (s).
    pub end_et_s: f64,
    /// Lap time (s) if both boundaries are known and plausible.
    pub time_s: Option<f64>,
    /// Source of `time_s`.
    pub time_source: LapTimeSource,
    /// Sector splits (s), `None` where unknown. Length = sector count at completion.
    pub sectors: Vec<Option<f64>>,
    /// Eligibility facts.
    pub flags: LapFlags,
    /// Official LMU time that disagreed with the computed time beyond tolerance (kept for diagnostics).
    pub official_mismatch_s: Option<f64>,
    /// Whether a complete distance trace was stored for the lap.
    pub trace_complete: bool,
}

impl LapRecord {
    /// The lap time is backed by enough evidence: start observed (or LMU lap start), two signals at each
    /// boundary that was observed, plausible duration.
    pub fn time_trusted(&self) -> bool {
        if self.flags.imported {
            return self.time_s.is_some();
        }
        let start_ok = if self.flags.joined_mid_lap {
            self.flags.start_evidence.lap_start_time
        } else {
            self.flags.start_evidence.count() >= MIN_BOUNDARY_EVIDENCE
        };
        start_ok
            && self.flags.end_evidence.count() >= MIN_BOUNDARY_EVIDENCE
            && self.time_s.is_some()
    }

    /// Eligible for best lap, best sectors and the personal-best reference: trusted time, not invalidated,
    /// not an in- or out-lap.
    pub fn eligible_for_best(&self) -> bool {
        self.time_trusted() && !self.flags.invalidated && !self.flags.pit_in && !self.flags.pit_out
    }

    /// The lap time as a metric whose provenance follows its source.
    pub fn time_metric(&self) -> Metric<f64> {
        match self.time_source {
            LapTimeSource::Official => Metric::measured("scoring.mLastLapTime", self.time_s),
            LapTimeSource::LmuLapStart | LapTimeSource::Imported => {
                Metric::derived(formula::LAP_TIME, self.time_s)
            }
            LapTimeSource::Interpolated => Metric::estimated(formula::LAP_TIME, self.time_s),
        }
    }
}

/// One `(distance, elapsed)` point of a lap trace.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TracePoint {
    /// Distance from the line (m).
    pub dist_m: f64,
    /// Elapsed time since the lap start (s).
    pub elapsed_s: f64,
}

/// Distance/elapsed trace of one complete lap, the basis of the distance-aligned delta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapTrace {
    /// Lap the trace belongs to.
    pub lap_id: LapId,
    /// Context of the lap.
    pub key: CompatibilityKey,
    /// Lap time (s); equals the elapsed time of the last point.
    pub lap_time_s: f64,
    /// Points with strictly increasing distance, starting at `(0, 0)`.
    pub points: Vec<TracePoint>,
}

impl LapTrace {
    /// Checks the trace invariants: at least [`MIN_TRACE_POINTS`] finite points, starting at `(0, 0)`,
    /// strictly increasing distance, non-decreasing elapsed time ending at the lap time.
    pub fn validate(&self) -> Result<(), TimingError> {
        if self.points.len() < MIN_TRACE_POINTS {
            return Err(TimingError::InvalidTrace("too few points"));
        }
        if self
            .points
            .iter()
            .any(|p| !p.dist_m.is_finite() || !p.elapsed_s.is_finite())
        {
            return Err(TimingError::InvalidTrace("non-finite point"));
        }
        let first = self.points[0];
        if first.dist_m != 0.0 || first.elapsed_s != 0.0 {
            return Err(TimingError::InvalidTrace("trace must start at (0, 0)"));
        }
        if self
            .points
            .windows(2)
            .any(|w| w[1].dist_m <= w[0].dist_m || w[1].elapsed_s < w[0].elapsed_s)
        {
            return Err(TimingError::InvalidTrace("trace must be monotonic"));
        }
        let last = self.points[self.points.len() - 1];
        if (last.elapsed_s - self.lap_time_s).abs() > f64::EPSILON * self.lap_time_s.max(1.0) {
            return Err(TimingError::InvalidTrace(
                "last point must be at the lap time",
            ));
        }
        Ok(())
    }

    /// Elapsed time at `dist_m`, linearly interpolated; `None` outside the trace.
    pub fn elapsed_at(&self, dist_m: f64) -> Option<f64> {
        if !dist_m.is_finite() {
            return None;
        }
        let idx = self.points.partition_point(|p| p.dist_m < dist_m);
        let hi = *self.points.get(idx)?;
        if hi.dist_m == dist_m {
            return Some(hi.elapsed_s);
        }
        let lo = *self.points.get(idx.checked_sub(1)?)?;
        Some(lerp(
            lo.dist_m,
            lo.elapsed_s,
            hi.dist_m,
            hi.elapsed_s,
            dist_m,
        ))
    }

    /// Distance covered by the trace (m).
    pub fn length_m(&self) -> f64 {
        self.points.last().map_or(0.0, |p| p.dist_m)
    }
}

/// Reference lap for the live delta (spec §18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceKind {
    /// Best compatible eligible lap with a complete trace.
    PersonalBest,
    /// Session best; distance-aligned only when the session best is one of our own laps.
    SessionBest,
    /// The lap completed just before the current one (any validity) with a complete trace.
    PreviousLap,
    /// A lap chosen by the engineer (own history or imported).
    SelectedLap(LapId),
}

/// Why a reference delta is or is not available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceStatus {
    /// The reference trace is available.
    Available,
    /// No suitable lap exists yet.
    NoLap,
    /// The lap exists but has no complete trace.
    NoTrace,
    /// The lap belongs to another track / vehicle / class / session.
    Incompatible,
    /// The session best belongs to another car; only its time is known.
    OtherCar,
}

/// Who holds the session best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionBestHolder {
    /// One of our laps.
    OwnLap(LapId),
    /// Another car (from scoring).
    OtherCar,
}

/// A completed sector split.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SectorSplit {
    /// Lap the sector belongs to.
    pub lap_id: LapId,
    /// Zero-based sector index.
    pub index: usize,
    /// Split time (s).
    pub time_s: f64,
}

/// Why the engine dropped its lap in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResetReason {
    /// The elapsed time went backwards (session restart, replay seek).
    EtRegression,
    /// The lap counter jumped by more than one or went backwards.
    LapCounterJump,
    /// The compatibility context (track, vehicle, class, session) changed.
    ContextChanged,
}

/// What happened on one update.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimingUpdate {
    /// Lap completed on this sample.
    pub completed_lap: Option<LapRecord>,
    /// Sectors completed on this sample (several with distance boundaries after a gap).
    pub completed_sectors: Vec<SectorSplit>,
    /// The lap in progress was abandoned (never completed or counted).
    pub abandoned_lap: Option<LapId>,
    /// Reset that happened on this sample.
    pub reset: Option<ResetReason>,
}

/// Timing state for display and publication.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimingSnapshot {
    /// LMU lap number of the lap in progress.
    pub lap_number: Option<i32>,
    /// Engine id of the lap in progress.
    pub current_lap_id: Option<LapId>,
    /// Laps completed (engine count, abandoned laps excluded).
    pub laps_completed: u32,
    /// Elapsed time of the lap in progress.
    pub current_lap_time: Metric<f64>,
    /// Current lap distance (m).
    pub lap_distance_m: Option<f64>,
    /// Lap progress 0..1.
    pub lap_progress: Option<f64>,
    /// Zero-based current sector.
    pub current_sector: Option<usize>,
    /// Number of sectors of the dynamic model.
    pub sector_count: usize,
    /// Splits of the lap in progress (completed sectors).
    pub current_sectors: Vec<Option<f64>>,
    /// Most recently completed sector split.
    pub previous_sector: Option<SectorSplit>,
    /// Last completed lap.
    pub last_lap: Option<LapRecord>,
    /// Personal best lap time (compatible eligible laps).
    pub personal_best: Metric<f64>,
    /// Lap holding the personal best.
    pub personal_best_lap: Option<LapId>,
    /// Session best lap time (own or other car's, from scoring).
    pub session_best: Metric<f64>,
    /// Holder of the session best.
    pub session_best_holder: Option<SessionBestHolder>,
    /// Best split per sector over compatible eligible laps.
    pub best_sectors: Vec<Metric<f64>>,
    /// Sum of the best sectors (all sectors needed).
    pub theoretical_best: Metric<f64>,
    /// Selected reference.
    pub reference: ReferenceKind,
    /// Lap used as the reference.
    pub reference_lap: Option<LapId>,
    /// Lap time of the reference.
    pub reference_lap_time: Option<f64>,
    /// Status of the selected reference.
    pub reference_status: ReferenceStatus,
    /// Distance-aligned delta to the selected reference (+ = slower).
    pub delta_to_reference: Metric<f64>,
    /// Distance-aligned delta to the personal best.
    pub delta_to_personal_best: Metric<f64>,
    /// Distance-aligned delta to the session best (only when it is our own lap).
    pub delta_to_session_best: Metric<f64>,
    /// Predicted lap: personal-best reference time plus the delta to it.
    pub predicted_lap: Metric<f64>,
    /// Completed sectors of the current lap minus the reference lap's sectors.
    pub sector_deltas_to_reference: Vec<Option<f64>>,
}

/// The previous sample's timing-relevant state.
#[derive(Debug, Clone, Copy)]
struct PrevSample {
    et_s: f64,
    lap_number: Option<i32>,
    lap_start_et_s: Option<f64>,
    dist_m: Option<f64>,
}

/// A lap boundary.
#[derive(Debug, Clone, Copy)]
struct Boundary {
    et_s: f64,
    source: BoundarySource,
    evidence: TransitionEvidence,
}

/// A transition whose evidence may still be completed by a late signal.
#[derive(Debug, Clone, Copy)]
struct RecentTransition {
    sample_et_s: f64,
    completed: Option<LapId>,
}

/// The lap in progress.
#[derive(Debug, Clone)]
struct CurrentLap {
    id: LapId,
    lap_number: Option<i32>,
    key: CompatibilityKey,
    start_et_s: Option<f64>,
    start_source: Option<BoundarySource>,
    start_sample_et_s: f64,
    joined_mid_lap: bool,
    start_evidence: TransitionEvidence,
    trace: Vec<TracePoint>,
    trace_gap: bool,
    max_dist_m: f64,
    live_point: Option<TracePoint>,
    sector: Option<usize>,
    sector_start_et_s: Option<f64>,
    sectors: Vec<Option<f64>>,
    sector_timing_broken: bool,
    invalidated: bool,
    pit_at_start: Option<bool>,
    outside_pit_seen: bool,
    pit_in: bool,
}

/// Cached bests over compatible laps.
#[derive(Debug, Clone, Default)]
struct Bests {
    personal_best: Option<(f64, LapId)>,
    personal_best_reference: Option<LapId>,
    best_sectors: Vec<Option<(f64, LapId)>>,
}

/// Lap and sector timing with distance-aligned references. Deterministic: the same samples always produce
/// the same laps, bests and deltas.
#[derive(Debug, Clone)]
pub struct TimingEngine {
    config: TimingConfig,
    key: CompatibilityKey,
    prev: Option<PrevSample>,
    current: Option<CurrentLap>,
    laps: VecDeque<LapRecord>,
    traces: BTreeMap<LapId, LapTrace>,
    next_id: u32,
    laps_completed: u32,
    reference: ReferenceKind,
    bests: Bests,
    previous_sector: Option<SectorSplit>,
    last_lap_last_sector: Option<usize>,
    recent: Option<RecentTransition>,
    last_wrap_et_s: Option<f64>,
    wrap_armed: bool,
    pending_official: Option<(LapId, f64)>,
    last_official_s: Option<f64>,
    external_session_best_s: Option<f64>,
    max_sector_index_seen: Option<usize>,
    track_length_m: Option<f64>,
}

impl TimingEngine {
    /// A new engine. Fails on an invalid sector model or trace resolution.
    pub fn new(config: TimingConfig) -> Result<Self, TimingError> {
        config.sector_model.validate()?;
        if !config.trace_resolution_m.is_finite() || config.trace_resolution_m <= 0.0 {
            return Err(TimingError::InvalidTraceResolution(
                config.trace_resolution_m,
            ));
        }
        Ok(Self {
            config,
            key: CompatibilityKey::default(),
            prev: None,
            current: None,
            laps: VecDeque::new(),
            traces: BTreeMap::new(),
            next_id: 0,
            laps_completed: 0,
            reference: ReferenceKind::PersonalBest,
            bests: Bests::default(),
            previous_sector: None,
            last_lap_last_sector: None,
            recent: None,
            last_wrap_et_s: None,
            wrap_armed: false,
            pending_official: None,
            last_official_s: None,
            external_session_best_s: None,
            max_sector_index_seen: None,
            track_length_m: None,
        })
    }

    /// The configuration.
    pub fn config(&self) -> &TimingConfig {
        &self.config
    }

    /// Current compatibility context.
    pub fn context(&self) -> &CompatibilityKey {
        &self.key
    }

    /// Sets the compatibility context. A change abandons the lap in progress (it was started under the
    /// old context) and recomputes bests; returns the reset when one happened.
    pub fn set_context(&mut self, key: CompatibilityKey) -> Option<ResetReason> {
        if key == self.key {
            return None;
        }
        debug!(component = COMPONENT, ?key, "timing context changed");
        self.key = key;
        self.abandon_current();
        self.prev = None;
        self.recent = None;
        self.last_wrap_et_s = None;
        self.wrap_armed = false;
        self.pending_official = None;
        self.last_official_s = None;
        self.max_sector_index_seen = None;
        self.recompute_bests();
        Some(ResetReason::ContextChanged)
    }

    /// Session best lap time of other cars (from scoring, e.g. the class best); `None` when unknown.
    pub fn set_external_session_best(&mut self, lap_s: Option<f64>) {
        self.external_session_best_s = finite(lap_s).filter(|t| *t >= MIN_LAP_TIME_S);
    }

    /// Selects the reference. A selected lap must exist, be compatible and have a complete trace.
    pub fn select_reference(&mut self, reference: ReferenceKind) -> Result<(), TimingError> {
        if let ReferenceKind::SelectedLap(id) = reference {
            let lap = self.lap(id).ok_or(TimingError::UnknownLap(id))?;
            if !lap
                .key
                .is_compatible(&self.key, self.config.allow_cross_session_references)
            {
                return Err(TimingError::IncompatibleLap(id));
            }
            if !self.traces.contains_key(&id) {
                return Err(TimingError::NoTrace(id));
            }
        }
        self.reference = reference;
        Ok(())
    }

    /// Selected reference.
    pub fn reference(&self) -> ReferenceKind {
        self.reference
    }

    /// Completed laps, oldest first (bounded by [`MAX_LAP_HISTORY`]).
    pub fn laps(&self) -> impl Iterator<Item = &LapRecord> {
        self.laps.iter()
    }

    /// Lap by id.
    pub fn lap(&self, id: LapId) -> Option<&LapRecord> {
        self.laps.iter().find(|l| l.id == id)
    }

    /// Stored trace of a lap.
    pub fn trace(&self, id: LapId) -> Option<&LapTrace> {
        self.traces.get(&id)
    }

    /// Number of sectors of the dynamic model (0 until known with the source-index model).
    pub fn sector_count(&self) -> usize {
        match &self.config.sector_model {
            SectorModel::SourceIndex => self.max_sector_index_seen.map_or(0, |m| m + 1),
            SectorModel::DistanceBoundaries(b) => b.len() + 1,
        }
    }

    /// Imports a lap with its trace (e.g. from a recording) so it can be selected as a reference. It is
    /// compared like any other lap: only within a compatible context.
    pub fn import_lap(
        &mut self,
        key: CompatibilityKey,
        lap_number: Option<i32>,
        sectors: Vec<Option<f64>>,
        lap_time_s: f64,
        points: Vec<TracePoint>,
    ) -> Result<LapId, TimingError> {
        if !(MIN_LAP_TIME_S..=MAX_LAP_TIME_S).contains(&lap_time_s) {
            return Err(TimingError::InvalidTrace("implausible lap time"));
        }
        if sectors.len() > MAX_SECTORS {
            return Err(TimingError::TooManySectors {
                count: sectors.len(),
            });
        }
        let id = self.allocate_id();
        let trace = LapTrace {
            lap_id: id,
            key: key.clone(),
            lap_time_s,
            points,
        };
        trace.validate()?;
        let record = LapRecord {
            id,
            lap_number,
            key,
            start_et_s: None,
            end_et_s: 0.0,
            time_s: Some(lap_time_s),
            time_source: LapTimeSource::Imported,
            sectors,
            flags: LapFlags {
                imported: true,
                ..LapFlags::default()
            },
            official_mismatch_s: None,
            trace_complete: true,
        };
        self.traces.insert(id, trace);
        self.push_record(record);
        Ok(id)
    }

    /// Feeds one sample. Samples must arrive in time order; a duplicate time is ignored and a time
    /// regression abandons the lap in progress.
    pub fn update(&mut self, s: &TimingSample) -> TimingUpdate {
        let mut out = TimingUpdate::default();
        if !s.et_s.is_finite() {
            debug!(
                component = COMPONENT,
                "sample without a finite time ignored"
            );
            return out;
        }
        if let Some(l) = finite(s.track_length_m).filter(|l| *l > 0.0) {
            self.track_length_m = Some(l);
        }
        if let Some(prev) = self.prev {
            if s.et_s == prev.et_s {
                return out;
            }
            if s.et_s < prev.et_s {
                warn!(
                    component = COMPONENT,
                    prev_et = prev.et_s,
                    et = s.et_s,
                    "elapsed time regression: lap in progress abandoned"
                );
                out.abandoned_lap = self.abandon_current();
                out.reset = Some(ResetReason::EtRegression);
                self.prev = None;
                self.recent = None;
                self.last_wrap_et_s = None;
                self.wrap_armed = false;
                self.pending_official = None;
            }
        }
        let prev = self.prev;
        let length = self.track_length_m;
        let dist = finite(s.lap_dist_m);

        let wrap_et = self.detect_wrap(prev.as_ref(), dist, length, s.et_s);
        if let Some(w) = wrap_et {
            self.last_wrap_et_s = Some(w);
        }
        let start_changed = lap_start_changed(prev.as_ref(), s);
        let counter = counter_step(prev.as_ref(), s);

        let transition = match counter {
            CounterStep::Increment => true,
            CounterStep::Same => false,
            CounterStep::Jump => {
                warn!(
                    component = COMPONENT,
                    lap_number = ?s.lap_number,
                    "lap counter jump: lap in progress abandoned"
                );
                out.abandoned_lap = self.abandon_current();
                out.reset = Some(ResetReason::LapCounterJump);
                self.recent = None;
                self.pending_official = None;
                false
            }
            CounterStep::Unavailable => {
                let start_available = prev.and_then(|p| p.lap_start_et_s).is_some()
                    && finite(s.lap_start_et_s).is_some();
                if start_available {
                    start_changed
                } else {
                    // Distance only: the wrap is the line crossing, unless the lap is implausibly short.
                    wrap_et.is_some()
                        && self.current.as_ref().is_none_or(|c| {
                            c.start_et_s.is_none_or(|st| s.et_s - st >= MIN_LAP_TIME_S)
                        })
                }
            }
        };

        if transition {
            let recent_wrap = self
                .last_wrap_et_s
                .is_some_and(|w| (s.et_s - w).abs() <= CONFIRMATION_WINDOW_S);
            let evidence = TransitionEvidence {
                lap_counter: counter == CounterStep::Increment,
                lap_start_time: start_changed,
                distance_wrap: recent_wrap,
            };
            if recent_wrap {
                // A wrap confirms one crossing only.
                self.last_wrap_et_s = None;
            }
            let boundary = match (start_changed, s.lap_start_et_s, wrap_et) {
                (true, Some(st), _) => Boundary {
                    et_s: st,
                    source: BoundarySource::LmuLapStart,
                    evidence,
                },
                (_, _, Some(w)) => Boundary {
                    et_s: w,
                    source: BoundarySource::Interpolated,
                    evidence,
                },
                _ => Boundary {
                    et_s: s.et_s,
                    source: BoundarySource::SampleTime,
                    evidence,
                },
            };
            let completed = self.complete_lap(boundary, &mut out);
            self.recent = Some(RecentTransition {
                sample_et_s: s.et_s,
                completed,
            });
            self.begin_lap(s, Some(boundary));
        } else {
            self.apply_late_evidence(s, wrap_et.is_some(), start_changed);
            if self.current.is_none() {
                self.begin_lap(s, None);
            }
        }

        // After the transition: an official time arriving with the line crossing belongs to the lap that
        // just completed, never to the one before it.
        self.adopt_official(s);
        self.track_in_lap(s, prev.as_ref(), dist, length, &mut out);
        self.prev = Some(PrevSample {
            et_s: s.et_s,
            lap_number: s.lap_number,
            lap_start_et_s: finite(s.lap_start_et_s),
            dist_m: dist,
        });
        out
    }

    /// Current timing state.
    pub fn snapshot(&self) -> TimingSnapshot {
        let cur = self.current.as_ref();
        let current_lap_time = cur
            .and_then(|c| Some(self.prev?.et_s - c.start_et_s?))
            .filter(|t| *t >= 0.0);
        let lap_distance_m = self.prev.and_then(|p| p.dist_m);
        let lap_progress = lap_distance_m
            .zip(self.track_length_m)
            .map(|(d, l)| (d / l).clamp(0.0, 1.0 - f64::EPSILON));
        let sector_count = self.sector_count();

        let pb_trace = self
            .bests
            .personal_best_reference
            .and_then(|id| self.traces.get(&id));
        let delta_pb = pb_trace.and_then(|t| self.live_delta(t));
        let predicted = pb_trace
            .zip(delta_pb)
            .map(|(t, d)| t.lap_time_s + d)
            .filter(|p| *p > 0.0);

        let (session_best, session_best_holder) = self.session_best();
        let sb_trace = match session_best_holder {
            Some(SessionBestHolder::OwnLap(_)) => pb_trace,
            _ => None,
        };
        let delta_sb = sb_trace.and_then(|t| self.live_delta(t));

        let (reference_lap, reference_status) = self.resolve_reference();
        let ref_trace = reference_lap
            .filter(|_| reference_status == ReferenceStatus::Available)
            .and_then(|id| self.traces.get(&id));
        let delta_ref = ref_trace.and_then(|t| self.live_delta(t));
        let ref_record = reference_lap.and_then(|id| self.lap(id));
        let sector_deltas = match (cur, ref_record) {
            (Some(c), Some(r)) => c
                .sectors
                .iter()
                .enumerate()
                .map(|(i, t)| Some((*t)? - r.sectors.get(i).copied().flatten()?))
                .collect(),
            _ => Vec::new(),
        };

        let best_sectors = (0..sector_count)
            .map(|i| {
                Metric::derived(
                    formula::SECTOR_TIME,
                    self.bests
                        .best_sectors
                        .get(i)
                        .copied()
                        .flatten()
                        .map(|b| b.0),
                )
            })
            .collect::<Vec<_>>();
        let theoretical = (sector_count > 0)
            .then(|| best_sectors.iter().map(|m| m.value).sum::<Option<f64>>())
            .flatten();

        TimingSnapshot {
            lap_number: cur.and_then(|c| c.lap_number),
            current_lap_id: cur.map(|c| c.id),
            laps_completed: self.laps_completed,
            current_lap_time: Metric::derived(formula::CURRENT_LAP_TIME, current_lap_time),
            lap_distance_m,
            lap_progress,
            current_sector: cur.and_then(|c| c.sector),
            sector_count,
            current_sectors: cur.map(|c| c.sectors.clone()).unwrap_or_default(),
            previous_sector: self.previous_sector,
            last_lap: self.laps.back().filter(|l| !l.flags.imported).cloned(),
            personal_best: Metric::derived(
                formula::BEST_LAP,
                self.bests.personal_best.map(|b| b.0),
            ),
            personal_best_lap: self.bests.personal_best.map(|b| b.1),
            session_best: Metric::derived(formula::BEST_LAP, session_best),
            session_best_holder,
            best_sectors,
            theoretical_best: Metric::derived(formula::THEORETICAL_BEST, theoretical),
            reference: self.reference,
            reference_lap,
            reference_lap_time: ref_record.and_then(|r| r.time_s),
            reference_status,
            delta_to_reference: Metric::derived(formula::LIVE_DELTA, delta_ref),
            delta_to_personal_best: Metric::derived(formula::LIVE_DELTA, delta_pb),
            delta_to_session_best: Metric::derived(formula::LIVE_DELTA, delta_sb),
            predicted_lap: Metric::estimated(formula::PREDICTED_LAP, predicted),
            sector_deltas_to_reference: sector_deltas,
        }
    }

    // ----- internals --------------------------------------------------------------------------------------

    fn allocate_id(&mut self) -> LapId {
        let id = LapId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Live delta of the lap in progress against `reference` at the current distance.
    fn live_delta(&self, reference: &LapTrace) -> Option<f64> {
        let cur = self.current.as_ref()?;
        if !reference
            .key
            .is_compatible(&cur.key, self.config.allow_cross_session_references)
        {
            return None;
        }
        let p = cur.live_point?;
        Some(p.elapsed_s - reference.elapsed_at(p.dist_m)?)
    }

    /// Session best time and holder.
    fn session_best(&self) -> (Option<f64>, Option<SessionBestHolder>) {
        let own = self.bests.personal_best;
        match (own, self.external_session_best_s) {
            (Some((t, id)), Some(ext)) if t <= ext => {
                (Some(t), Some(SessionBestHolder::OwnLap(id)))
            }
            (_, Some(ext)) => (Some(ext), Some(SessionBestHolder::OtherCar)),
            (Some((t, id)), None) => (Some(t), Some(SessionBestHolder::OwnLap(id))),
            (None, None) => (None, None),
        }
    }

    /// Lap and status of the selected reference.
    fn resolve_reference(&self) -> (Option<LapId>, ReferenceStatus) {
        let allow = self.config.allow_cross_session_references;
        let status_of = |id: LapId| -> ReferenceStatus {
            match self.lap(id) {
                None => ReferenceStatus::NoLap,
                Some(l) if !l.key.is_compatible(&self.key, allow) => ReferenceStatus::Incompatible,
                Some(_) if !self.traces.contains_key(&id) => ReferenceStatus::NoTrace,
                Some(_) => ReferenceStatus::Available,
            }
        };
        match self.reference {
            ReferenceKind::PersonalBest => match self.bests.personal_best_reference {
                Some(id) => (Some(id), status_of(id)),
                None => (
                    None,
                    if self.bests.personal_best.is_some() {
                        ReferenceStatus::NoTrace
                    } else {
                        ReferenceStatus::NoLap
                    },
                ),
            },
            ReferenceKind::SessionBest => match self.session_best().1 {
                Some(SessionBestHolder::OwnLap(_)) => match self.bests.personal_best_reference {
                    Some(id) => (Some(id), status_of(id)),
                    None => (None, ReferenceStatus::NoTrace),
                },
                Some(SessionBestHolder::OtherCar) => (None, ReferenceStatus::OtherCar),
                None => (None, ReferenceStatus::NoLap),
            },
            ReferenceKind::PreviousLap => {
                match self.laps.iter().rev().find(|l| !l.flags.imported) {
                    Some(l) => (Some(l.id), status_of(l.id)),
                    None => (None, ReferenceStatus::NoLap),
                }
            }
            ReferenceKind::SelectedLap(id) => (Some(id), status_of(id)),
        }
    }

    /// Detects a distance wrap and returns the interpolated crossing time.
    fn detect_wrap(
        &mut self,
        prev: Option<&PrevSample>,
        dist: Option<f64>,
        length: Option<f64>,
        et_s: f64,
    ) -> Option<f64> {
        let (d, l) = (dist?, length?);
        if d > WRAP_REARM_FRACTION * l {
            self.wrap_armed = true;
        }
        let p = prev?;
        let pd = p.dist_m?;
        if !(self.wrap_armed && pd > WRAP_HIGH_FRACTION * l && d < WRAP_LOW_FRACTION * l) {
            return None;
        }
        self.wrap_armed = false;
        // Crossing where the distance reaches the line: the part of the step before the line is (l − pd),
        // the part after it is d (both clamped at 0 for samples slightly past the nominal length or before 0).
        let before = (l - pd).max(0.0);
        let after = d.max(0.0);
        let frac = if before + after > 0.0 {
            before / (before + after)
        } else {
            1.0
        };
        Some(p.et_s + frac * (et_s - p.et_s))
    }

    /// Starts a lap at `boundary`, or joins the lap in progress when `boundary` is `None`.
    fn begin_lap(&mut self, s: &TimingSample, boundary: Option<Boundary>) {
        let id = self.allocate_id();
        let (start_et_s, start_source, joined, evidence) = match boundary {
            Some(b) => (Some(b.et_s), Some(b.source), false, b.evidence),
            None => {
                // Joined mid-lap: LMU's lap start still times the lap correctly when it is plausible.
                let lmu_start = finite(s.lap_start_et_s)
                    .filter(|st| *st <= s.et_s && s.et_s - *st <= MAX_LAP_TIME_S);
                let evidence = TransitionEvidence {
                    lap_start_time: lmu_start.is_some(),
                    ..TransitionEvidence::default()
                };
                (
                    lmu_start,
                    lmu_start.map(|_| BoundarySource::LmuLapStart),
                    true,
                    evidence,
                )
            }
        };
        let sector_count = self.sector_count();
        let mut lap = CurrentLap {
            id,
            lap_number: s.lap_number,
            key: self.key.clone(),
            start_et_s,
            start_source,
            start_sample_et_s: s.et_s,
            joined_mid_lap: joined,
            start_evidence: evidence,
            trace: Vec::new(),
            trace_gap: false,
            max_dist_m: 0.0,
            live_point: None,
            sector: None,
            sector_start_et_s: None,
            sectors: vec![None; sector_count],
            sector_timing_broken: false,
            invalidated: false,
            pit_at_start: None,
            outside_pit_seen: false,
            pit_in: false,
        };
        if !joined {
            lap.trace.push(TracePoint {
                dist_m: 0.0,
                elapsed_s: 0.0,
            });
            lap.sector = Some(0);
            lap.sector_start_et_s = start_et_s;
        }
        self.current = Some(lap);
    }

    /// Drops the lap in progress without completing it.
    fn abandon_current(&mut self) -> Option<LapId> {
        let lap = self.current.take()?;
        debug!(component = COMPONENT, lap = lap.id.0, "lap abandoned");
        Some(lap.id)
    }

    /// Completes the lap in progress at `boundary`.
    fn complete_lap(&mut self, boundary: Boundary, out: &mut TimingUpdate) -> Option<LapId> {
        let mut cur = self.current.take()?;
        // The last sector ends at the line.
        if let (Some(j), Some(st)) = (cur.sector, cur.sector_start_et_s) {
            let split = boundary.et_s - st;
            if split > 0.0 {
                ensure_len(&mut cur.sectors, j + 1);
                cur.sectors[j] = Some(split);
                let ss = SectorSplit {
                    lap_id: cur.id,
                    index: j,
                    time_s: split,
                };
                self.previous_sector = Some(ss);
                out.completed_sectors.push(ss);
            }
        }
        self.last_lap_last_sector = cur.sector;
        let time_s = cur
            .start_et_s
            .map(|st| boundary.et_s - st)
            .filter(|t| (MIN_LAP_TIME_S..=MAX_LAP_TIME_S).contains(t));
        let time_source = match (cur.start_source, boundary.source) {
            (Some(BoundarySource::LmuLapStart), BoundarySource::LmuLapStart) => {
                LapTimeSource::LmuLapStart
            }
            _ => LapTimeSource::Interpolated,
        };
        // Close the trace at the line: distance = track length, elapsed = lap time.
        let mut trace_complete = false;
        if let (Some(t), Some(l)) = (time_s, self.track_length_m) {
            let last_d = cur.trace.last().map_or(0.0, |p| p.dist_m);
            if !cur.joined_mid_lap && !cur.trace_gap && l > last_d {
                if l - last_d > MAX_TRACE_GAP_M {
                    cur.trace_gap = true;
                } else {
                    cur.trace.push(TracePoint {
                        dist_m: l,
                        elapsed_s: t,
                    });
                    trace_complete = cur.trace.len() >= MIN_TRACE_POINTS;
                }
            }
        }
        let record = LapRecord {
            id: cur.id,
            lap_number: cur.lap_number,
            key: cur.key.clone(),
            start_et_s: cur.start_et_s,
            end_et_s: boundary.et_s,
            time_s,
            time_source,
            sectors: cur.sectors.clone(),
            flags: LapFlags {
                invalidated: cur.invalidated,
                pit_in: cur.pit_in,
                pit_out: cur.pit_at_start == Some(true),
                joined_mid_lap: cur.joined_mid_lap,
                trace_gap: cur.trace_gap,
                sector_timing_broken: cur.sector_timing_broken,
                start_evidence: cur.start_evidence,
                end_evidence: boundary.evidence,
                imported: false,
            },
            official_mismatch_s: None,
            trace_complete,
        };
        if trace_complete && let Some(t) = time_s {
            self.traces.insert(
                cur.id,
                LapTrace {
                    lap_id: cur.id,
                    key: cur.key,
                    lap_time_s: t,
                    points: cur.trace,
                },
            );
        }
        debug!(
            component = COMPONENT,
            lap = record.id.0,
            lap_number = ?record.lap_number,
            time = ?record.time_s,
            evidence = record.flags.end_evidence.count(),
            "lap completed"
        );
        self.laps_completed = self.laps_completed.saturating_add(1);
        self.pending_official = Some((record.id, boundary.et_s));
        out.completed_lap = Some(record.clone());
        let id = record.id;
        self.push_record(record);
        Some(id)
    }

    /// Appends a record, enforces the history bound and refreshes bests and traces.
    fn push_record(&mut self, record: LapRecord) {
        self.laps.push_back(record);
        while self.laps.len() > MAX_LAP_HISTORY {
            if let Some(old) = self.laps.pop_front() {
                self.traces.remove(&old.id);
            }
        }
        self.recompute_bests();
        self.evict_traces();
    }

    /// Late evidence for the most recent transition (scoring-derived distance lags the counter).
    fn apply_late_evidence(&mut self, s: &TimingSample, wrapped: bool, start_changed: bool) {
        let Some(recent) = self.recent else {
            return;
        };
        if s.et_s - recent.sample_et_s > CONFIRMATION_WINDOW_S {
            self.recent = None;
            return;
        }
        let mut changed = false;
        if wrapped {
            if let Some(cur) = self.current.as_mut()
                && !cur.start_evidence.distance_wrap
            {
                cur.start_evidence.distance_wrap = true;
                changed = true;
            }
            if let Some(id) = recent.completed
                && let Some(lap) = self.laps.iter_mut().find(|l| l.id == id)
                && !lap.flags.end_evidence.distance_wrap
            {
                lap.flags.end_evidence.distance_wrap = true;
                changed = true;
            }
            if changed {
                self.last_wrap_et_s = None;
            }
        }
        if start_changed && let Some(new_start) = finite(s.lap_start_et_s) {
            changed |= self.correct_start(recent.completed, new_start);
        }
        if changed {
            self.recompute_bests();
        }
    }

    /// Moves the most recent boundary to LMU's lap start time reported after the transition.
    fn correct_start(&mut self, completed: Option<LapId>, new_start: f64) -> bool {
        let Some(cur) = self.current.as_mut() else {
            return false;
        };
        if cur.start_evidence.lap_start_time || cur.joined_mid_lap {
            return false;
        }
        let Some(old_start) = cur.start_et_s else {
            return false;
        };
        let shift = old_start - new_start;
        if shift.abs() > CONFIRMATION_WINDOW_S {
            return false;
        }
        cur.start_et_s = Some(new_start);
        cur.start_source = Some(BoundarySource::LmuLapStart);
        cur.start_evidence.lap_start_time = true;
        if cur.sector_start_et_s == Some(old_start) {
            cur.sector_start_et_s = Some(new_start);
        }
        // elapsed = et − start, so every elapsed value grows by (old_start − new_start); the anchor stays.
        for p in cur.trace.iter_mut().skip(1) {
            p.elapsed_s += shift;
        }
        if let Some(lp) = cur.live_point.as_mut() {
            lp.elapsed_s += shift;
        }
        if let Some(id) = completed
            && let Some(lap) = self.laps.iter_mut().find(|l| l.id == id)
        {
            lap.end_et_s = new_start;
            lap.flags.end_evidence.lap_start_time = true;
            lap.time_s = lap
                .start_et_s
                .map(|st| new_start - st)
                .filter(|t| (MIN_LAP_TIME_S..=MAX_LAP_TIME_S).contains(t));
            if lap.time_source != LapTimeSource::Official {
                lap.time_source = LapTimeSource::Interpolated;
            }
            if let Some(trace) = self.traces.get_mut(&id) {
                match lap.time_s {
                    Some(t) => {
                        trace.lap_time_s = t;
                        if let Some(last) = trace.points.last_mut() {
                            last.elapsed_s = t;
                        }
                        if trace.validate().is_err() {
                            self.traces.remove(&id);
                            lap.trace_complete = false;
                        }
                    }
                    None => {
                        self.traces.remove(&id);
                        lap.trace_complete = false;
                    }
                }
            }
        }
        true
    }

    /// Adopts an official LMU last-lap time for the lap just completed when it matches.
    fn adopt_official(&mut self, s: &TimingSample) {
        let Some(official) = finite(s.official_last_lap_s).filter(|t| *t > 0.0) else {
            return;
        };
        let is_new = self.last_official_s != Some(official);
        self.last_official_s = Some(official);
        if !is_new {
            return;
        }
        let Some((id, completed_et)) = self.pending_official else {
            return;
        };
        if s.et_s - completed_et > OFFICIAL_LAP_ADOPTION_WINDOW_S {
            self.pending_official = None;
            return;
        }
        self.pending_official = None;
        let Some(lap) = self.laps.iter_mut().find(|l| l.id == id) else {
            return;
        };
        match lap.time_s {
            Some(t) if (t - official).abs() <= OFFICIAL_LAP_TIME_TOLERANCE_S => {
                lap.time_s = Some(official);
                lap.time_source = LapTimeSource::Official;
                if let Some(trace) = self.traces.get_mut(&id) {
                    // Rescale elapsed times so the trace ends exactly at the official time.
                    let scale = official / trace.lap_time_s;
                    for p in &mut trace.points {
                        p.elapsed_s *= scale;
                    }
                    trace.lap_time_s = official;
                    if let Some(last) = trace.points.last_mut() {
                        last.elapsed_s = official;
                    }
                }
                self.recompute_bests();
            }
            other => {
                warn!(
                    component = COMPONENT,
                    lap = id.0,
                    computed = ?other,
                    official,
                    "official lap time disagrees with the computed time; computed time kept"
                );
                lap.official_mismatch_s = Some(official);
            }
        }
    }

    /// Per-sample tracking inside the lap: sectors, trace, flags.
    fn track_in_lap(
        &mut self,
        s: &TimingSample,
        prev: Option<&PrevSample>,
        dist: Option<f64>,
        length: Option<f64>,
        out: &mut TimingUpdate,
    ) {
        let resolution = self.config.trace_resolution_m;
        let model = &self.config.sector_model;
        let last_lap_last_sector = self.last_lap_last_sector;
        if let (SectorModel::SourceIndex, Some(k)) = (model, s.sector_index) {
            let k = usize::from(k);
            if k < MAX_SECTORS {
                self.max_sector_index_seen =
                    Some(self.max_sector_index_seen.map_or(k, |m| m.max(k)));
            }
        }
        let Some(cur) = self.current.as_mut() else {
            return;
        };
        let elapsed = cur.start_et_s.map(|st| s.et_s - st);
        let in_lag_window = s.et_s - cur.start_sample_et_s <= CONFIRMATION_WINDOW_S;

        // A distance that still belongs to the previous lap (start of a lap) or already to the next one
        // (end of a lap, the scoring-derived distance wrapped before the counter) is not this lap's.
        let usable_dist = match (dist, length) {
            (Some(d), Some(l)) => {
                let stale_end_of_previous =
                    !cur.joined_mid_lap && in_lag_window && d > WRAP_HIGH_FRACTION * l;
                let early_wrap =
                    cur.max_dist_m > WRAP_HIGH_FRACTION * l && d < WRAP_LOW_FRACTION * l;
                (!stale_end_of_previous && !early_wrap && d >= 0.0).then_some(d)
            }
            (Some(d), None) => (d >= 0.0).then_some(d),
            _ => None,
        };

        // Sectors.
        match model {
            SectorModel::SourceIndex => {
                if let Some(k) = s.sector_index.map(usize::from).filter(|k| *k < MAX_SECTORS) {
                    let mid = prev.map_or(s.et_s, |p| 0.5 * (p.et_s + s.et_s));
                    match cur.sector {
                        Some(j) if k == j => {}
                        Some(j) if k == j + 1 => {
                            if let Some(st) = cur.sector_start_et_s {
                                let split = mid - st;
                                if split > 0.0 {
                                    ensure_len(&mut cur.sectors, j + 1);
                                    cur.sectors[j] = Some(split);
                                    let ss = SectorSplit {
                                        lap_id: cur.id,
                                        index: j,
                                        time_s: split,
                                    };
                                    self.previous_sector = Some(ss);
                                    out.completed_sectors.push(ss);
                                }
                            }
                            cur.sector = Some(k);
                            cur.sector_start_et_s = Some(mid);
                        }
                        Some(_) if in_lag_window && Some(k) == last_lap_last_sector => {
                            // Stale index of the previous lap's last sector right after the line.
                        }
                        Some(_) if k == 0 => {
                            // Line reached: the lap transition closes the last sector.
                        }
                        Some(_) => {
                            cur.sector_timing_broken = true;
                            cur.sector = Some(k);
                            cur.sector_start_et_s = None;
                        }
                        None => {
                            cur.sector = Some(k);
                            cur.sector_start_et_s = None;
                        }
                    }
                    ensure_len(&mut cur.sectors, k + 1);
                }
            }
            SectorModel::DistanceBoundaries(bounds) => {
                if let Some(d) = usable_dist {
                    let k = bounds.partition_point(|b| *b <= d);
                    match cur.sector {
                        Some(j) if k > j => {
                            let p = prev.and_then(|p| p.dist_m.map(|pd| (pd, p.et_s)));
                            for (idx, b) in bounds.iter().enumerate().take(k).skip(j) {
                                let cross = match p {
                                    Some((pd, pet))
                                        if pd <= *b && *b <= d && d - pd <= MAX_TRACE_GAP_M =>
                                    {
                                        Some(lerp(pd, pet, d, s.et_s, *b))
                                    }
                                    _ => None,
                                };
                                match (cross, cur.sector_start_et_s) {
                                    (Some(c), Some(st)) if c > st => {
                                        ensure_len(&mut cur.sectors, idx + 1);
                                        cur.sectors[idx] = Some(c - st);
                                        let ss = SectorSplit {
                                            lap_id: cur.id,
                                            index: idx,
                                            time_s: c - st,
                                        };
                                        self.previous_sector = Some(ss);
                                        out.completed_sectors.push(ss);
                                    }
                                    (None, _) => cur.sector_timing_broken = true,
                                    _ => {}
                                }
                                cur.sector_start_et_s = cross;
                            }
                            cur.sector = Some(k);
                        }
                        Some(_) => {}
                        None => {
                            cur.sector = Some(k);
                            cur.sector_start_et_s = None;
                        }
                    }
                }
            }
        }

        // Trace and live point.
        if let (Some(d), Some(e)) = (usable_dist, elapsed) {
            cur.live_point = Some(TracePoint {
                dist_m: d,
                elapsed_s: e,
            });
            cur.max_dist_m = cur.max_dist_m.max(d);
            if !cur.joined_mid_lap && !cur.trace_gap {
                let last = cur.trace.last().copied();
                match last {
                    Some(lp) if d >= lp.dist_m + resolution => {
                        if d - lp.dist_m > MAX_TRACE_GAP_M || cur.trace.len() >= MAX_TRACE_POINTS {
                            cur.trace_gap = true;
                        } else if e >= lp.elapsed_s {
                            cur.trace.push(TracePoint {
                                dist_m: d,
                                elapsed_s: e,
                            });
                        }
                    }
                    Some(_) => {}
                    None => cur.trace.push(TracePoint {
                        dist_m: d,
                        elapsed_s: e,
                    }),
                }
            }
        } else if usable_dist.is_none() {
            cur.live_point = None;
        }

        // Flags.
        if s.lap_invalidated == Some(true) {
            cur.invalidated = true;
        }
        if let Some(pit) = s.in_pit_lane {
            if cur.pit_at_start.is_none() {
                cur.pit_at_start = Some(pit);
            }
            if pit {
                if cur.outside_pit_seen {
                    cur.pit_in = true;
                }
            } else {
                cur.outside_pit_seen = true;
            }
        }
    }

    /// Recomputes bests over compatible laps.
    fn recompute_bests(&mut self) {
        let allow = self.config.allow_cross_session_references;
        let sector_count = self.sector_count();
        let mut bests = Bests {
            best_sectors: vec![None; sector_count],
            ..Bests::default()
        };
        for lap in self
            .laps
            .iter()
            .filter(|l| l.key.is_compatible(&self.key, allow) && l.eligible_for_best())
        {
            let Some(t) = lap.time_s else { continue };
            if bests.personal_best.is_none_or(|(b, _)| t < b) {
                bests.personal_best = Some((t, lap.id));
            }
            if self.traces.contains_key(&lap.id)
                && bests
                    .personal_best_reference
                    .and_then(|id| self.lap(id).and_then(|l| l.time_s))
                    .is_none_or(|b| t < b)
            {
                bests.personal_best_reference = Some(lap.id);
            }
            if lap.flags.sector_timing_broken || lap.sectors.len() != sector_count {
                continue;
            }
            for (slot, split) in bests.best_sectors.iter_mut().zip(&lap.sectors) {
                if let Some(st) = split
                    && slot.is_none_or(|(b, _)| *st < b)
                {
                    *slot = Some((*st, lap.id));
                }
            }
        }
        self.bests = bests;
    }

    /// Keeps at most [`MAX_STORED_TRACES`] traces, never evicting the pinned references.
    fn evict_traces(&mut self) {
        let mut pinned = BTreeSet::new();
        if let Some(id) = self.bests.personal_best_reference {
            pinned.insert(id);
        }
        if let Some(l) = self.laps.back() {
            pinned.insert(l.id);
        }
        if let ReferenceKind::SelectedLap(id) = self.reference {
            pinned.insert(id);
        }
        while self.traces.len() > MAX_STORED_TRACES {
            let Some(victim) = self.traces.keys().find(|id| !pinned.contains(id)).copied() else {
                break;
            };
            self.traces.remove(&victim);
            if let Some(l) = self.laps.iter_mut().find(|l| l.id == victim) {
                l.trace_complete = false;
            }
        }
    }
}

/// How the lap counter moved between two samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CounterStep {
    Increment,
    Same,
    Jump,
    Unavailable,
}

fn counter_step(prev: Option<&PrevSample>, s: &TimingSample) -> CounterStep {
    match (prev.and_then(|p| p.lap_number), s.lap_number) {
        (Some(p), Some(c)) if c == p => CounterStep::Same,
        (Some(p), Some(c)) if p.checked_add(1) == Some(c) => CounterStep::Increment,
        (Some(_), Some(_)) => CounterStep::Jump,
        _ => CounterStep::Unavailable,
    }
}

fn lap_start_changed(prev: Option<&PrevSample>, s: &TimingSample) -> bool {
    match (
        prev.and_then(|p| p.lap_start_et_s),
        finite(s.lap_start_et_s),
    ) {
        (Some(a), Some(b)) => {
            b > a + MIN_LAP_TIME_S
                && b <= s.et_s + LAP_START_ET_TOLERANCE_S
                && s.et_s - b <= LAP_START_ET_TOLERANCE_S
        }
        _ => false,
    }
}

fn ensure_len(v: &mut Vec<Option<f64>>, len: usize) {
    if v.len() < len {
        v.resize(len, None);
    }
}

#[cfg(test)]
mod tests;

//! Rates and timing from capture timestamps: update intervals per block, gaps, ET monotonicity, duplicate
//! frames, telemetry-vs-scoring ET offset and ET vs capture-clock drift.
//!
//! Intervals are measured on the capture clock (`capture_mono_ns`, the capturing reader's monotonic clock
//! taken right after each copy) and never across a reconnection. ET is LMU's player `mElapsedTime`.

use std::collections::BTreeMap;

use lmu_normalizer::discontinuity::{
    CarSample, LMU_GAME_PHASE_SESSION_OVER, TELEPORT_ET_STEP_BACK_MAX_S, is_session_end_rewind,
    is_teleport_step_back,
};
use lmu_normalizer::mapping::decode_current_sector;
use lmu_normalizer::mapping::session_end_et;
use lmu_normalizer::{NormalizedOutput, NormalizerStats, TelemetryDisposition};
use lmu_reader::{RawSnapshot, ReaderStats};
use serde::Serialize;

use crate::stats::{DurationHistogram, Summary, SummaryStats, TopK};

/// Nanoseconds per second.
pub const NS_PER_S: f64 = 1e9;
/// Nanoseconds per millisecond.
const NS_PER_MS: f64 = 1e6;
/// An interval longer than this many medians is reported as a gap.
pub const GAP_FACTOR: f64 = 3.0;
/// Largest gaps listed per stream.
const LISTED_GAPS: usize = 20;
/// A drift segment ends at a telemetry interval longer than this (pause, menu, stall).
const SEGMENT_BREAK_NS: u64 = 500_000_000;
/// Shortest segment whose drift is reported (s).
const MIN_SEGMENT_S: f64 = 5.0;
/// Drift segments listed individually (the rest are only aggregated).
const LISTED_SEGMENTS: usize = 20;
/// Regressions recorded for classification; beyond this they count as unexplained (conservative).
const MAX_RECORDED_REGRESSIONS: usize = 1_000;
/// Regressions listed in the report.
const LISTED_REGRESSIONS: usize = 50;
/// Identity changes remembered for explaining regressions.
const MAX_IDENTITY_CHANGES: usize = 1_000;
/// A regression to an ET below this is a restart from (near) zero, i.e. a new session (s).
pub const RESTART_ET_MAX_S: f64 = 10.0;
/// A regression within this capture time of an identity change is explained by it (s).
const REGRESSION_IDENTITY_WINDOW_S: f64 = 2.0;
/// Plausible `mDeltaTime` values recorded in the histogram are below this (s).
const MAX_DELTA_TIME_S: f64 = 1.0;
/// Parts per million.
const PPM: f64 = 1e6;

/// Interval statistics in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct IntervalStats {
    /// Intervals measured.
    pub count: u64,
    /// Shortest.
    pub min_ms: f64,
    /// 5th percentile.
    pub p5_ms: f64,
    /// Median.
    pub median_ms: f64,
    /// 95th percentile.
    pub p95_ms: f64,
    /// Longest.
    pub max_ms: f64,
    /// Mean.
    pub mean_ms: f64,
}

impl IntervalStats {
    fn of(h: &DurationHistogram) -> Option<Self> {
        let ms = |ns: f64| ns / NS_PER_MS;
        Some(Self {
            count: h.count(),
            min_ms: ms(h.min_ns()? as f64),
            p5_ms: ms(h.quantile_ns(0.05)?),
            median_ms: ms(h.quantile_ns(0.5)?),
            p95_ms: ms(h.quantile_ns(0.95)?),
            max_ms: ms(h.max_ns()? as f64),
            mean_ms: ms(h.mean_ns()?),
        })
    }
}

/// One long interval between updates.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Gap {
    /// Capture time at the end of the gap (s since the first snapshot).
    pub at_capture_s: f64,
    /// Interval length (ms).
    pub length_ms: f64,
    /// Player ET before the gap (s).
    pub et_before_s: Option<f64>,
    /// Player ET after the gap (s).
    pub et_after_s: Option<f64>,
}

/// Update statistics of one block.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamReport {
    /// Updates seen.
    pub updates: u64,
    /// `1000 / median interval`.
    pub median_rate_hz: Option<f64>,
    /// Intervals ÷ their total duration.
    pub mean_rate_hz: Option<f64>,
    /// Interval distribution.
    pub interval_ms: Option<IntervalStats>,
    /// Gap threshold: [`GAP_FACTOR`] × median (ms).
    pub gap_threshold_ms: Option<f64>,
    /// Intervals above the threshold (histogram resolution 0.2 %).
    pub gaps: u64,
    /// The longest intervals above the threshold (at most 20).
    pub largest_gaps: Vec<Gap>,
}

#[derive(Debug, Clone)]
struct UpdateStream {
    count: u64,
    last_ns: Option<u64>,
    last_et: Option<f64>,
    intervals: DurationHistogram,
    gaps: TopK<Gap>,
}

impl Default for UpdateStream {
    fn default() -> Self {
        Self {
            count: 0,
            last_ns: None,
            last_et: None,
            intervals: DurationHistogram::default(),
            gaps: TopK::new(LISTED_GAPS),
        }
    }
}

impl UpdateStream {
    /// Records an update; returns the interval since the previous one (ns).
    fn record(&mut self, now_ns: u64, capture_s: f64, et: Option<f64>) -> Option<u64> {
        self.count += 1;
        let interval = self.last_ns.map(|last| now_ns.saturating_sub(last));
        if let Some(ns) = interval {
            self.intervals.record(ns);
            self.gaps.push(
                ns as f64,
                Gap {
                    at_capture_s: capture_s,
                    length_ms: ns as f64 / NS_PER_MS,
                    et_before_s: self.last_et,
                    et_after_s: et,
                },
            );
        }
        self.last_ns = Some(now_ns);
        self.last_et = et;
        interval
    }

    /// Forgets the previous update (reconnection): no interval spans two connections.
    fn break_chain(&mut self) {
        self.last_ns = None;
        self.last_et = None;
    }

    fn report(&self) -> StreamReport {
        let interval_ms = IntervalStats::of(&self.intervals);
        let median_ns = self.intervals.quantile_ns(0.5);
        let threshold_ns = median_ns.map(|m| m * GAP_FACTOR);
        let total_s = self
            .intervals
            .mean_ns()
            .map(|m| m * self.intervals.count() as f64 / NS_PER_S);
        StreamReport {
            updates: self.count,
            median_rate_hz: median_ns.filter(|m| *m > 0.0).map(|m| NS_PER_S / m),
            mean_rate_hz: total_s
                .filter(|t| *t > 0.0)
                .map(|t| self.intervals.count() as f64 / t),
            interval_ms,
            gap_threshold_ms: threshold_ns.map(|t| t / NS_PER_MS),
            gaps: threshold_ns.map_or(0, |t| self.intervals.count_above(t)),
            largest_gaps: self
                .gaps
                .sorted()
                .into_iter()
                .filter(|(ns, _)| threshold_ns.is_some_and(|t| *ns > t))
                .map(|(_, g)| g.clone())
                .collect(),
        }
    }
}

/// A player ET regression.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EtRegression {
    /// Capture time (s).
    pub at_capture_s: f64,
    /// ET before (s).
    pub from_et_s: f64,
    /// ET after (s).
    pub to_et_s: f64,
    /// How the regression is explained, if it is.
    pub explanation: Option<String>,
    /// The session was over, the player out of realtime and the new ET at or after the session end: LMU's
    /// session-end rewind (`lmu_normalizer::discontinuity::is_session_end_rewind`, A-LMU-031).
    pub session_end_rewind: bool,
    /// What the normalizer did with the sample that stepped back.
    pub normalizer: NormalizerHandling,
    /// The step back is a garage teleport by `lmu_normalizer::discontinuity::is_teleport_step_back` against the
    /// previous sample (position jump, at rest, in the pit area, ET back by at most
    /// [`TELEPORT_ET_STEP_BACK_MAX_S`]): evidence only, the classification follows the normalizer.
    pub teleport_step_back_geometry: bool,
}

impl EtRegression {
    /// The explanation, or `UNEXPLAINED (normalizer: …)`.
    pub fn describe(&self) -> String {
        self.explanation
            .clone()
            .unwrap_or_else(|| format!("UNEXPLAINED (normalizer: {})", self.normalizer.label()))
    }
}

/// How the normalizer handled the sample of an ET regression, from its disposition and the change of its
/// counters across that snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NormalizerHandling {
    /// Dropped as a garage-teleport ET step-back (`teleport_et_step_backs` incremented): same session, no
    /// restart event.
    TeleportStepBackDropped,
    /// Session-end rewind: time base reset, no restart event.
    SessionEndRewind,
    /// Session restart: state reset and a `SessionChanged` event (a false restart unless a new session began).
    Restart,
    /// Anything else (the normalizer did not see a regression on this sample).
    Other,
}

impl NormalizerHandling {
    /// Report label.
    pub fn label(self) -> &'static str {
        match self {
            Self::TeleportStepBackDropped => "dropped as a teleport ET step-back",
            Self::SessionEndRewind => "session-end rewind",
            Self::Restart => "session restart, SessionChanged emitted",
            Self::Other => "no regression handling",
        }
    }

    fn of(disposition: TelemetryDisposition, delta: NormalizerDelta) -> Self {
        match disposition {
            TelemetryDisposition::Duplicate if delta.teleport_et_step_back => {
                Self::TeleportStepBackDropped
            }
            TelemetryDisposition::NewSampleAfterRegression if delta.session_end_rewind => {
                Self::SessionEndRewind
            }
            TelemetryDisposition::NewSampleAfterRegression => Self::Restart,
            _ => Self::Other,
        }
    }
}

/// Normalizer counter changes across one snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NormalizerDelta {
    /// `teleport_et_step_backs` grew.
    pub teleport_et_step_back: bool,
    /// `session_end_rewinds` grew.
    pub session_end_rewind: bool,
    /// `teleports` grew: the normalizer recognised a garage teleport on this snapshot (a jump at rest into
    /// the pit area, `lmu_normalizer::discontinuity::is_teleport`, including one whose ET stepped back).
    /// Gate 1 pitstop1's return to the garage is a 19.17 m jump, far below a plain distance threshold.
    pub teleport: bool,
}

impl NormalizerDelta {
    /// The changes from `before` to `after`.
    pub fn between(before: &NormalizerStats, after: &NormalizerStats) -> Self {
        Self {
            teleport_et_step_back: after.teleport_et_step_backs > before.teleport_et_step_backs,
            session_end_rewind: after.session_end_rewinds > before.session_end_rewinds,
            teleport: after.teleports > before.teleports,
        }
    }

    /// The car was moved by LMU on this snapshot: a garage teleport or a teleport ET step-back.
    pub fn moved_by_lmu(&self) -> bool {
        self.teleport || self.teleport_et_step_back
    }
}

/// The normalizer's session-end rewind rule applied to a step back from `prev_et_s` to `new_et_s`.
fn session_end_rewind(info: &lmu_reader::RawScoringInfo, prev_et_s: f64, new_et_s: f64) -> bool {
    is_session_end_rewind(
        info.game_phase,
        info.in_realtime,
        session_end_et(info.end_et),
        prev_et_s,
        new_et_s,
    )
}

/// Drift of one continuous running segment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DriftSegment {
    /// Capture time at the segment start (s).
    pub start_capture_s: f64,
    /// Segment length on the capture clock (s).
    pub duration_s: f64,
    /// Telemetry updates in the segment.
    pub samples: u64,
    /// `(dET/dcapture − 1) × 10⁶`: positive = ET runs faster than the capture clock.
    pub drift_ppm: f64,
    /// RMS residual of ET around the fitted line (ms).
    pub residual_rms_ms: f64,
}

/// ET vs capture-clock drift over all segments.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DriftReport {
    /// Segments of at least 5 s (a segment ends at a telemetry gap > 500 ms, an ET regression or a reconnect).
    pub segments: u64,
    /// Total duration of those segments (s).
    pub total_s: f64,
    /// Duration-weighted mean drift (ppm).
    pub weighted_drift_ppm: Option<f64>,
    /// Smallest segment drift (ppm).
    pub min_drift_ppm: Option<f64>,
    /// Largest segment drift (ppm).
    pub max_drift_ppm: Option<f64>,
    /// Per-update |ΔET − Δcapture| inside segments (ms).
    pub step_deviation_ms: Option<IntervalStats>,
    /// The first 20 segments.
    pub listed: Vec<DriftSegment>,
}

/// Running least-squares fit of ET (y) against capture time (x), both relative to the segment start.
#[derive(Debug, Clone)]
struct Segment {
    x0_ns: u64,
    y0: f64,
    start_capture_s: f64,
    n: u64,
    sx: f64,
    sy: f64,
    sxx: f64,
    sxy: f64,
    syy: f64,
    last_x_ns: u64,
    last_y: f64,
}

impl Segment {
    fn start(x_ns: u64, y: f64, capture_s: f64) -> Self {
        let mut s = Self {
            x0_ns: x_ns,
            y0: y,
            start_capture_s: capture_s,
            n: 0,
            sx: 0.0,
            sy: 0.0,
            sxx: 0.0,
            sxy: 0.0,
            syy: 0.0,
            last_x_ns: x_ns,
            last_y: y,
        };
        s.add(x_ns, y);
        s
    }

    fn add(&mut self, x_ns: u64, y: f64) {
        let x = x_ns.saturating_sub(self.x0_ns) as f64 / NS_PER_S;
        let y_rel = y - self.y0;
        self.n += 1;
        self.sx += x;
        self.sy += y_rel;
        self.sxx += x * x;
        self.sxy += x * y_rel;
        self.syy += y_rel * y_rel;
        self.last_x_ns = x_ns;
        self.last_y = y;
    }

    fn result(&self) -> Option<DriftSegment> {
        let duration_s = self.last_x_ns.saturating_sub(self.x0_ns) as f64 / NS_PER_S;
        if self.n < 3 || duration_s < MIN_SEGMENT_S {
            return None;
        }
        let n = self.n as f64;
        let sxx = self.sxx - self.sx * self.sx / n;
        let sxy = self.sxy - self.sx * self.sy / n;
        let syy = self.syy - self.sy * self.sy / n;
        if sxx <= 0.0 {
            return None;
        }
        let slope = sxy / sxx;
        let ssr = (syy - slope * sxy).max(0.0);
        Some(DriftSegment {
            start_capture_s: self.start_capture_s,
            duration_s,
            samples: self.n,
            drift_ppm: (slope - 1.0) * PPM,
            residual_rms_ms: (ssr / n).sqrt() * 1e3,
        })
    }
}

#[derive(Debug, Clone, Default)]
struct DriftTracker {
    current: Option<Segment>,
    segments: u64,
    total_s: f64,
    weighted_sum: f64,
    min_ppm: Option<f64>,
    max_ppm: Option<f64>,
    step_dev: DurationHistogram,
    listed: Vec<DriftSegment>,
}

impl DriftTracker {
    fn sample(&mut self, x_ns: u64, et: f64, capture_s: f64) {
        let deviation_s = self.current.as_ref().and_then(|seg| {
            let gap_ns = x_ns.saturating_sub(seg.last_x_ns);
            (gap_ns <= SEGMENT_BREAK_NS && et > seg.last_y)
                .then(|| ((et - seg.last_y) - gap_ns as f64 / NS_PER_S).abs())
        });
        match (deviation_s, self.current.as_mut()) {
            (Some(dev), Some(seg)) => {
                seg.add(x_ns, et);
                // ET steps are positive and capture steps at most 500 ms here; a deviation of more than
                // u64 nanoseconds is impossible, the cast saturates harmlessly otherwise.
                self.step_dev.record((dev * NS_PER_S) as u64);
            }
            _ => {
                self.close();
                self.current = Some(Segment::start(x_ns, et, capture_s));
            }
        }
    }

    fn close(&mut self) {
        if let Some(result) = self.current.take().and_then(|s| s.result()) {
            self.segments += 1;
            self.total_s += result.duration_s;
            self.weighted_sum += result.drift_ppm * result.duration_s;
            self.min_ppm = Some(
                self.min_ppm
                    .map_or(result.drift_ppm, |m| m.min(result.drift_ppm)),
            );
            self.max_ppm = Some(
                self.max_ppm
                    .map_or(result.drift_ppm, |m| m.max(result.drift_ppm)),
            );
            if self.listed.len() < LISTED_SEGMENTS {
                self.listed.push(result);
            }
        }
    }

    fn report(mut self) -> DriftReport {
        self.close();
        DriftReport {
            segments: self.segments,
            total_s: self.total_s,
            weighted_drift_ppm: (self.total_s > 0.0).then(|| self.weighted_sum / self.total_s),
            min_drift_ppm: self.min_ppm,
            max_drift_ppm: self.max_ppm,
            step_deviation_ms: IntervalStats::of(&self.step_dev),
            listed: self.listed,
        }
    }
}

/// Counters copied from the replay's `ReaderStats`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ReaderCounters {
    /// Snapshots the processor returned.
    pub snapshots: u64,
    /// Frames byte-identical to the previous one (dropped by the processor).
    pub byte_identical_duplicates: u64,
    /// Snapshots where neither telemetry nor scoring changed (generic block only).
    pub generic_only_frames: u64,
    /// Frames rejected by structural checks.
    pub rejected_frames: u64,
    /// Player ET regressions counted by the reader.
    pub telemetry_et_regressions: u64,
    /// ET steps longer than 1.5 × `mDeltaTime`.
    pub telemetry_gaps: u64,
    /// Samples LMU published but the capture never saw (inside those gaps).
    pub telemetry_samples_skipped: u64,
    /// Snapshots whose player was found by `mID` in another slot.
    pub player_found_by_id_frames: u64,
    /// Snapshots whose player could not be verified (player unavailable).
    pub player_unverified_frames: u64,
}

impl ReaderCounters {
    /// Copies the counters of a replay.
    pub fn of(stats: &ReaderStats) -> Self {
        Self {
            snapshots: stats.snapshots,
            byte_identical_duplicates: stats.duplicate_frames,
            generic_only_frames: stats.same_et_frames,
            rejected_frames: stats.rejected_frames,
            telemetry_et_regressions: stats.telemetry_et_regressions,
            telemetry_gaps: stats.telemetry_gaps,
            telemetry_samples_skipped: stats.telemetry_samples_skipped,
            player_found_by_id_frames: stats.player_found_by_id_frames,
            player_unverified_frames: stats.player_unverified_frames,
        }
    }
}

/// Everything in the "Rates & timing" section.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimingReport {
    /// First → last snapshot on the capture clock (s).
    pub capture_duration_s: f64,
    /// Snapshots replayed.
    pub snapshots: u64,
    /// Connections (LMU attaches) in the capture.
    pub connections: u64,
    /// Snapshots with a new player telemetry sample.
    pub telemetry: StreamReport,
    /// Snapshots with a new scoring block.
    pub scoring: StreamReport,
    /// Every snapshot.
    pub all_snapshots: StreamReport,
    /// Player ET step between consecutive telemetry updates (same car).
    pub et_step_ms: Option<IntervalStats>,
    /// Raw `mDeltaTime` of fresh samples.
    pub delta_time_ms: Option<IntervalStats>,
    /// Player ET regressions (same car).
    pub et_regressions: u64,
    /// Regressions not explained by a session-end rewind, a restart near ET 0, an identity change or a
    /// garage-teleport step-back the normalizer dropped.
    pub unexplained_et_regressions: u64,
    /// The recorded regressions (first 50).
    pub regressions: Vec<EtRegression>,
    /// Scoring `mCurrentET` regressions.
    pub scoring_et_regressions: u64,
    /// Scoring `mCurrentET` regressions to an ET of at least [`RESTART_ET_MAX_S`] (not a session restart).
    pub scoring_et_regressions_not_restart: u64,
    /// Scoring `mCurrentET` regressions that are a session-end rewind (not counted in
    /// `scoring_et_regressions_not_restart`).
    #[serde(default)]
    pub scoring_et_session_end_rewinds: u64,
    /// Scoring updates whose `mCurrentET` was NaN or infinite.
    pub scoring_et_non_finite: u64,
    /// Normalizer telemetry dispositions per snapshot.
    pub dispositions: BTreeMap<String, u64>,
    /// Reader counters of the replay.
    pub reader: ReaderCounters,
    /// Player telemetry ET − scoring `mCurrentET` at scoring updates (s).
    pub telemetry_minus_scoring_et_s: Option<SummaryStats>,
    /// |telemetry ET − scoring ET| at scoring updates (s).
    pub abs_telemetry_minus_scoring_et_s: Option<SummaryStats>,
    /// ET vs capture-clock drift.
    pub drift: DriftReport,
}

/// The player slot / car of a telemetry sample.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PlayerEt {
    slot: u8,
    id: i32,
    et: f64,
}

/// Rates and timing accumulator.
#[derive(Debug, Clone, Default)]
pub struct TimingAcc {
    first_ns: Option<u64>,
    last_ns: Option<u64>,
    snapshots: u64,
    connections: u64,
    connection: Option<u64>,
    telemetry: UpdateStream,
    scoring: UpdateStream,
    all: UpdateStream,
    et_step: DurationHistogram,
    delta_time: DurationHistogram,
    last_player: Option<PlayerEt>,
    regressions: Vec<EtRegression>,
    regression_count: u64,
    scoring_et_last: Option<f64>,
    scoring_regressions: u64,
    scoring_regressions_not_restart: u64,
    scoring_session_end_rewinds: u64,
    scoring_non_finite: u64,
    drift: DriftTracker,
    offset: Summary,
    abs_offset: Summary,
    dispositions: BTreeMap<&'static str, u64>,
    identity_changes_s: Vec<f64>,
    /// Slot, id and position facts of the last player sample (teleport step-back evidence).
    last_player_pos: Option<(u8, i32, CarSample)>,
}

/// Label of a normalizer disposition.
fn disposition_label(d: TelemetryDisposition) -> &'static str {
    match d {
        TelemetryDisposition::NewSample => "NEW_SAMPLE",
        TelemetryDisposition::NewSampleAfterRegression => "NEW_SAMPLE_AFTER_REGRESSION",
        TelemetryDisposition::Duplicate => "DUPLICATE_ET",
        TelemetryDisposition::NoPlayer => "NO_PLAYER",
        TelemetryDisposition::NonFiniteEt => "NON_FINITE_ET",
    }
}

impl TimingAcc {
    /// Capture time of `ns` relative to the first snapshot (s).
    pub fn capture_s(&self, ns: u64) -> f64 {
        self.first_ns
            .map_or(0.0, |first| ns.saturating_sub(first) as f64 / NS_PER_S)
    }

    /// Feeds one snapshot, its normalizer output and the normalizer counter changes it caused.
    pub fn observe(
        &mut self,
        snapshot: &RawSnapshot,
        out: &NormalizedOutput,
        delta: NormalizerDelta,
    ) {
        let now = snapshot.capture_mono_ns;
        self.first_ns.get_or_insert(now);
        self.last_ns = Some(now);
        self.snapshots += 1;
        if self.connection != Some(snapshot.connection_id) {
            if self.connection.is_some() {
                self.telemetry.break_chain();
                self.scoring.break_chain();
                self.all.break_chain();
                self.last_player = None;
                self.scoring_et_last = None;
                self.drift.close();
            }
            self.connections += 1;
            self.connection = Some(snapshot.connection_id);
        }
        let capture_s = self.capture_s(now);
        let player_et = snapshot.player_et().filter(|e| e.is_finite());
        self.all.record(now, capture_s, player_et);
        *self
            .dispositions
            .entry(disposition_label(out.disposition))
            .or_insert(0) += 1;
        if out.identity.is_some() && self.identity_changes_s.len() < MAX_IDENTITY_CHANGES {
            self.identity_changes_s.push(capture_s);
        }

        if snapshot.telemetry_updated {
            self.telemetry.record(now, capture_s, player_et);
            if let Some(p) = &snapshot.player {
                // The pit area as the normalizer sees it: the pit-lane bit, or the player's scoring row (joined
                // by `mID`) in the pits or in its garage stall.
                let row = snapshot
                    .scoring
                    .vehicles
                    .iter()
                    .find(|r| r.is_player && r.id == p.telemetry.id);
                let (_, pit_lane_bit) = decode_current_sector(p.telemetry.current_sector);
                let in_pit_area =
                    pit_lane_bit || row.is_some_and(|r| r.in_pits || r.in_garage_stall);
                self.observe_player(
                    now,
                    capture_s,
                    p.slot,
                    &p.telemetry,
                    in_pit_area,
                    &snapshot.scoring.info,
                    NormalizerHandling::of(out.disposition, delta),
                );
            }
        }
        if snapshot.scoring_updated {
            let s_et = snapshot.scoring.info.current_et;
            let s_et = s_et.is_finite().then_some(s_et);
            self.scoring.record(now, capture_s, player_et);
            match s_et {
                None => self.scoring_non_finite += 1,
                Some(e) => {
                    if let Some(prev) = self.scoring_et_last.filter(|&prev| e < prev) {
                        self.scoring_regressions += 1;
                        if session_end_rewind(&snapshot.scoring.info, prev, e) {
                            self.scoring_session_end_rewinds += 1;
                        } else if e >= RESTART_ET_MAX_S {
                            self.scoring_regressions_not_restart += 1;
                        }
                    }
                    self.scoring_et_last = Some(e);
                    if let Some(p) = player_et {
                        self.offset.push(p - e);
                        self.abs_offset.push((p - e).abs());
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn observe_player(
        &mut self,
        now: u64,
        capture_s: f64,
        slot: u8,
        t: &lmu_reader::RawTelemetry,
        in_pit_area: bool,
        info: &lmu_reader::RawScoringInfo,
        handling: NormalizerHandling,
    ) {
        let dt = t.delta_time;
        if dt.is_finite() && dt > 0.0 && dt < MAX_DELTA_TIME_S {
            // Bounded above, so the cast cannot saturate.
            self.delta_time.record((dt * NS_PER_S) as u64);
        }
        let sample = CarSample {
            et_s: t.elapsed_time,
            pos: [t.pos.x, t.pos.y, t.pos.z],
            speed_mps: lmu_normalizer::convention::speed_mps(t.local_vel),
            in_pit_area,
        };
        let geometry = self.teleport_step_back_geometry(slot, t.id, sample);
        let et = t.elapsed_time;
        if !et.is_finite() {
            return;
        }
        let current = PlayerEt { slot, id: t.id, et };
        match self.last_player {
            Some(last) if last.slot == slot && last.id == t.id => {
                if et > last.et {
                    let step = et - last.et;
                    if step < f64::from(u32::MAX) {
                        self.et_step.record((step * NS_PER_S) as u64);
                    }
                    self.drift.sample(now, et, capture_s);
                } else if et < last.et {
                    self.regression_count += 1;
                    if self.regressions.len() < MAX_RECORDED_REGRESSIONS {
                        self.regressions.push(EtRegression {
                            at_capture_s: capture_s,
                            from_et_s: last.et,
                            to_et_s: et,
                            explanation: None,
                            session_end_rewind: session_end_rewind(info, last.et, et),
                            normalizer: handling,
                            teleport_step_back_geometry: geometry,
                        });
                    }
                    self.drift.close();
                    self.drift.sample(now, et, capture_s);
                }
            }
            _ => {
                self.drift.close();
                self.drift.sample(now, et, capture_s);
            }
        }
        self.last_player = Some(current);
    }

    /// `true` when `sample` is a garage teleport with a small ET step-back from the previous sample of the
    /// same car (`lmu_normalizer::discontinuity::is_teleport_step_back`); remembers `sample` for the next call.
    fn teleport_step_back_geometry(&mut self, slot: u8, id: i32, sample: CarSample) -> bool {
        let geometry = self
            .last_player_pos
            .is_some_and(|(last_slot, last_id, prev)| {
                last_slot == slot && last_id == id && is_teleport_step_back(&prev, &sample)
            });
        if sample.pos.iter().all(|v| v.is_finite()) && sample.et_s.is_finite() {
            self.last_player_pos = Some((slot, id, sample));
        }
        geometry
    }

    /// Builds the report.
    pub fn finish(mut self, reader: &ReaderStats) -> TimingReport {
        let changes = std::mem::take(&mut self.identity_changes_s);
        for r in &mut self.regressions {
            r.explanation = if r.session_end_rewind {
                Some(format!(
                    "session-end rewind: the session was over (mGamePhase {LMU_GAME_PHASE_SESSION_OVER}), the player out of realtime, and ET went back to at or after mEndET (LMU closing the session, A-LMU-031)"
                ))
            } else if r.to_et_s < RESTART_ET_MAX_S {
                Some(format!(
                    "ET restarted below {RESTART_ET_MAX_S} s (new session)"
                ))
            } else if changes
                .iter()
                .any(|&c| (c - r.at_capture_s).abs() <= REGRESSION_IDENTITY_WINDOW_S)
            {
                Some("session/track/vehicle identity changed within 2 s".to_owned())
            } else if r.normalizer == NormalizerHandling::TeleportStepBackDropped
                && r.from_et_s - r.to_et_s <= TELEPORT_ET_STEP_BACK_MAX_S
            {
                // Classified by what the normalizer did, never by a looser copy of its rule: any other
                // regression to a non-zero ET is a full restart (SessionChanged) in the product.
                let how = if r.teleport_step_back_geometry {
                    "the car jumped into the garage on this sample"
                } else {
                    "continuing a step-back after a garage teleport"
                };
                Some(format!(
                    "garage-teleport ET step-back of ≤ {TELEPORT_ET_STEP_BACK_MAX_S} s dropped by the normalizer ({how}; same session, not a restart, A-LMU-031)"
                ))
            } else {
                None
            };
        }
        let recorded = self.regressions.len() as u64;
        let unexplained_recorded = self
            .regressions
            .iter()
            .filter(|r| r.explanation.is_none())
            .count() as u64;
        // Regressions beyond the recording limit cannot be classified: counted as unexplained.
        let unexplained = unexplained_recorded + self.regression_count.saturating_sub(recorded);
        let duration_s = match (self.first_ns, self.last_ns) {
            (Some(a), Some(b)) => b.saturating_sub(a) as f64 / NS_PER_S,
            _ => 0.0,
        };
        let listed_regressions: Vec<EtRegression> = self
            .regressions
            .iter()
            .take(LISTED_REGRESSIONS)
            .cloned()
            .collect();
        TimingReport {
            capture_duration_s: duration_s,
            snapshots: self.snapshots,
            connections: self.connections,
            telemetry: self.telemetry.report(),
            scoring: self.scoring.report(),
            all_snapshots: self.all.report(),
            et_step_ms: IntervalStats::of(&self.et_step),
            delta_time_ms: IntervalStats::of(&self.delta_time),
            et_regressions: self.regression_count,
            unexplained_et_regressions: unexplained,
            regressions: listed_regressions,
            scoring_et_regressions: self.scoring_regressions,
            scoring_et_regressions_not_restart: self.scoring_regressions_not_restart,
            scoring_et_session_end_rewinds: self.scoring_session_end_rewinds,
            scoring_et_non_finite: self.scoring_non_finite,
            dispositions: self
                .dispositions
                .iter()
                .map(|(k, v)| ((*k).to_owned(), *v))
                .collect(),
            reader: ReaderCounters::of(reader),
            telemetry_minus_scoring_et_s: self.offset.stats(),
            abs_telemetry_minus_scoring_et_s: self.abs_offset.stats(),
            drift: self.drift.report(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car(et_s: f64, x: f64, speed_mps: f64, in_pit_area: bool) -> CarSample {
        CarSample {
            et_s,
            pos: [x, 0.0, 0.0],
            speed_mps,
            in_pit_area,
        }
    }

    /// Step-back geometry is the normalizer's `is_teleport_step_back`, per car.
    #[test]
    fn teleport_step_back_geometry_follows_the_normalizer_rule() {
        let geometry = |samples: &[CarSample]| {
            let mut acc = TimingAcc::default();
            samples
                .iter()
                .map(|s| acc.teleport_step_back_geometry(0, 7, *s))
                .collect::<Vec<_>>()
        };
        // Jump into the garage with ET 10 ms back on the same sample.
        assert_eq!(
            geometry(&[car(315.830, 0.0, 0.0, true), car(315.820, 19.6, 0.0, true)]),
            [false, true]
        );
        // pitstop1: the jump lands on an advancing sample, the step-back on the next one (no jump).
        assert_eq!(
            geometry(&[
                car(315.820, 0.0, 0.0, true),
                car(315.830, 19.6, 0.0, true),
                car(315.820, 19.6, 0.0, true)
            ]),
            [false, false, false]
        );
        // Out on track it is never a teleport.
        assert_eq!(
            geometry(&[
                car(315.830, 0.0, 0.0, false),
                car(315.820, 19.6, 0.0, false)
            ]),
            [false, false]
        );
        // Another car (different id) is never compared.
        let mut acc = TimingAcc::default();
        assert!(!acc.teleport_step_back_geometry(0, 7, car(1.01, 0.0, 0.0, true)));
        assert!(!acc.teleport_step_back_geometry(0, 8, car(1.0, 19.6, 0.0, true)));
    }

    fn regression(from: f64, to: f64, normalizer: NormalizerHandling) -> EtRegression {
        EtRegression {
            at_capture_s: 100.0,
            from_et_s: from,
            to_et_s: to,
            explanation: None,
            session_end_rewind: false,
            normalizer,
            teleport_step_back_geometry: false,
        }
    }

    fn unexplained(r: EtRegression) -> u64 {
        let acc = TimingAcc {
            regression_count: 1,
            regressions: vec![r],
            ..TimingAcc::default()
        };
        acc.finish(&ReaderStats::default())
            .unexplained_et_regressions
    }

    /// Second review F2: the real pitstop1 step-back (315.830 → 315.820 after a garage teleport) is explained
    /// only when the normalizer dropped it; a session restart at ET 315 s is a false restart.
    #[test]
    fn step_backs_are_classified_by_what_the_normalizer_did() {
        use NormalizerHandling::{Other, Restart, SessionEndRewind, TeleportStepBackDropped};
        assert_eq!(unexplained(regression(315.830, 315.820, Restart)), 1);
        assert_eq!(
            unexplained(regression(315.830, 315.820, TeleportStepBackDropped)),
            0
        );
        assert_eq!(unexplained(regression(315.830, 315.820, Other)), 1);
        assert_eq!(
            unexplained(regression(315.830, 315.820, SessionEndRewind)),
            1
        );
        // Beyond the normalizer's step-back bound nothing is excused, whatever the disposition says.
        assert_eq!(
            unexplained(regression(315.830, 315.230, TeleportStepBackDropped)),
            1
        );
        // A restart from near zero stays a new session.
        assert_eq!(unexplained(regression(315.830, 1.0, Restart)), 0);
    }

    /// The teleport flag follows the normalizer's `teleports` counter; a dropped step-back alone also
    /// counts as the car being moved by LMU.
    #[test]
    fn normalizer_delta_reports_teleports() {
        let before = NormalizerStats::default();
        let jumped = NormalizerStats {
            teleports: 1,
            ..before
        };
        let d = NormalizerDelta::between(&before, &jumped);
        assert!(d.teleport && !d.teleport_et_step_back && d.moved_by_lmu());
        let stepped_back = NormalizerStats {
            teleport_et_step_backs: 1,
            ..jumped
        };
        let d = NormalizerDelta::between(&jumped, &stepped_back);
        assert!(!d.teleport && d.teleport_et_step_back && d.moved_by_lmu());
        assert!(!NormalizerDelta::between(&jumped, &jumped).moved_by_lmu());
    }

    #[test]
    fn normalizer_handling_needs_the_counter_and_the_disposition() {
        let dropped = NormalizerDelta {
            teleport_et_step_back: true,
            ..NormalizerDelta::default()
        };
        assert_eq!(
            NormalizerHandling::of(TelemetryDisposition::Duplicate, dropped),
            NormalizerHandling::TeleportStepBackDropped
        );
        assert_eq!(
            NormalizerHandling::of(TelemetryDisposition::Duplicate, NormalizerDelta::default()),
            NormalizerHandling::Other
        );
        assert_eq!(
            NormalizerHandling::of(
                TelemetryDisposition::NewSampleAfterRegression,
                NormalizerDelta::default()
            ),
            NormalizerHandling::Restart
        );
        let rewind = NormalizerDelta {
            session_end_rewind: true,
            ..NormalizerDelta::default()
        };
        assert_eq!(
            NormalizerHandling::of(TelemetryDisposition::NewSampleAfterRegression, rewind),
            NormalizerHandling::SessionEndRewind
        );
    }

    #[test]
    fn drift_segment_fits_a_line() {
        let mut d = DriftTracker::default();
        // ET runs 100 ppm fast relative to the capture clock for 20 s at 100 Hz.
        for i in 0..2_000u64 {
            let x = i * 10_000_000;
            let et = 50.0 + (x as f64 / NS_PER_S) * (1.0 + 100e-6);
            d.sample(x, et, x as f64 / NS_PER_S);
        }
        // A pause (gap > 500 ms) starts a new segment.
        for i in 0..1_000u64 {
            let x = 30_000_000_000 + i * 10_000_000;
            d.sample(x, 100.0 + i as f64 * 0.01, x as f64 / NS_PER_S);
        }
        let r = d.report();
        assert_eq!(r.segments, 2);
        let first = r.listed.first().expect("segment");
        assert!((first.drift_ppm - 100.0).abs() < 0.5, "{first:?}");
        assert!(first.residual_rms_ms < 1e-3);
        assert!(r.listed.get(1).is_some_and(|s| s.drift_ppm.abs() < 0.5));
        assert!(r.weighted_drift_ppm.is_some_and(|w| w > 0.0 && w < 100.0));
    }

    #[test]
    fn stream_gaps_use_the_median() {
        let mut s = UpdateStream::default();
        let mut t = 0u64;
        for i in 0..200u64 {
            t += if i == 100 { 2_000_000_000 } else { 10_000_000 };
            s.record(t, t as f64 / NS_PER_S, Some(t as f64 / NS_PER_S));
        }
        let r = s.report();
        assert_eq!(r.updates, 200);
        assert_eq!(r.gaps, 1);
        assert_eq!(r.largest_gaps.len(), 1);
        assert!((r.largest_gaps[0].length_ms - 2_000.0).abs() < 1e-9);
        assert!(r.median_rate_hz.is_some_and(|hz| (hz - 100.0).abs() < 0.5));
    }
}

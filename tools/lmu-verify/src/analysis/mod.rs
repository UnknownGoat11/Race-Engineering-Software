//! Replays one capture through `lmu-reader` and `lmu-normalizer` (exactly as the live app does) and feeds
//! every snapshot to the streaming accumulators of the report sections.
//!
//! A capture that cannot be opened or stops early (truncated, corrupt, incompatible frame) never aborts the
//! run: the report says where it stopped and analyses everything before that point.

pub mod census;
pub mod channels;
pub mod checks;
pub mod session;
pub mod timing;

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use lmu_normalizer::{NormalizedOutput, Normalizer, NormalizerStats, TelemetryDisposition};
use lmu_reader::compat::CompatTracker;
use lmu_reader::{
    CaptureError, CaptureReader, CaptureReplay, CompatibilityReport, RawSnapshot, RawTelemetry,
    RawVehicleScoring, ReaderStats,
};
use serde::Serialize;
use telemetry_model::frame::TelemetryFrame;

use crate::COMPONENT;
use crate::redact::{NameCollector, Redactor};

use self::census::Census;
use self::channels::{ChannelRow, ChannelsAcc};
use self::checks::{CheckResult, ChecksAcc};
use self::session::{SessionAcc, SessionFacts};
use self::timing::{NS_PER_S, TimingAcc, TimingReport};

/// The normalizer's measured rates are refreshed from the replay statistics this often (capture time), as
/// the live app refreshes them periodically.
const RATE_REFRESH_NS: u64 = 1_000_000_000;

/// Where and why a replay stopped before the end of the file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StopInfo {
    /// Zero-based index of the frame record that could not be read (`None`: the header).
    pub frame_index: Option<u64>,
    /// Snapshots analysed before the stop.
    pub snapshots_before: u64,
    /// Capture time of the last analysed snapshot (s since the first one).
    pub last_capture_s: Option<f64>,
    /// Error category (`TRUNCATED`, `CORRUPT`, `INCOMPATIBLE`, ...).
    pub kind: String,
    /// The reader's error message (redacted).
    pub message: String,
}

/// Replay outcome.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReplayStatus {
    /// The whole file was replayed.
    pub complete: bool,
    /// Frame records read from the file.
    pub frames_read: u64,
    /// Snapshots produced (byte-identical frames are dropped by the processor).
    pub snapshots: u64,
    /// Set when the replay stopped early.
    pub stopped: Option<StopInfo>,
}

/// The report of one capture.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaptureReport {
    /// File name (no directories: they may contain the user name).
    pub file: String,
    /// File size (bytes), when known.
    pub file_bytes: Option<u64>,
    /// Replay outcome.
    pub replay: ReplayStatus,
    /// Session facts.
    pub session: SessionFacts,
    /// Rates and timing (`None` when no snapshot was replayed).
    pub timing: Option<TimingReport>,
    /// Per-channel table (registry order; every channel that appeared in a published frame).
    pub channels: Vec<ChannelRow>,
    /// Raw NaN / Â±Inf census.
    pub census: Census,
    /// Gate-1 checks (fixed order).
    pub checks: Vec<CheckResult>,
    /// Normalizer counters at the end of the replay.
    pub normalizer: NormalizerStats,
}

/// One normalizer step as seen by the accumulators.
#[derive(Debug, Clone, Copy)]
pub struct Step<'a> {
    /// The snapshot.
    pub snapshot: &'a RawSnapshot,
    /// What the normalizer produced for it.
    pub out: &'a NormalizedOutput,
    /// Capture time relative to the first snapshot (s).
    pub capture_s: f64,
    /// The new physics frame, if the snapshot carried a new player sample.
    pub frame: Option<&'a TelemetryFrame>,
    /// The raw player telemetry of that new sample (only together with `frame`).
    pub raw: Option<&'a RawTelemetry>,
    /// The player's scoring row (`mIsPlayer` with the player telemetry `mID`).
    pub player_row: Option<&'a RawVehicleScoring>,
    /// Continuity broke before this step (reconnect, ET regression, session/track/vehicle change):
    /// differences with earlier samples are meaningless.
    pub discontinuity: bool,
}

/// All accumulators of one capture.
#[derive(Debug)]
pub struct CaptureAnalysis {
    normalizer: Normalizer,
    session: SessionAcc,
    timing: TimingAcc,
    channels: ChannelsAcc,
    census: Census,
    checks: ChecksAcc,
    last_connection: Option<u64>,
    last_rate_refresh_ns: Option<u64>,
    snapshots: u64,
}

impl CaptureAnalysis {
    /// Fresh accumulators for a capture with header text `meta`.
    pub fn new(meta: &str) -> Self {
        Self {
            normalizer: Normalizer::new(),
            session: SessionAcc::new(meta),
            timing: TimingAcc::default(),
            channels: ChannelsAcc::new(),
            census: Census::default(),
            checks: ChecksAcc::default(),
            last_connection: None,
            last_rate_refresh_ns: None,
            snapshots: 0,
        }
    }

    /// Normalizes and analyses one snapshot; `stats` are the replay statistics so far.
    pub fn observe(&mut self, snapshot: &RawSnapshot, stats: &ReaderStats) {
        self.snapshots += 1;
        let now = snapshot.capture_mono_ns;
        if self
            .last_rate_refresh_ns
            .is_none_or(|last| now.saturating_sub(last) >= RATE_REFRESH_NS)
        {
            self.normalizer.update_rates_from_stats(stats);
            self.last_rate_refresh_ns = Some(now);
        }
        let out = self.normalizer.process(snapshot);
        let reconnected = self
            .last_connection
            .is_some_and(|c| c != snapshot.connection_id);
        self.last_connection = Some(snapshot.connection_id);
        let discontinuity = reconnected
            || out.identity.is_some()
            || out.disposition == TelemetryDisposition::NewSampleAfterRegression;

        self.timing.observe(snapshot, &out);
        let capture_s = self.timing.capture_s(now);
        self.session.observe(snapshot, &out, capture_s);
        if snapshot.scoring_updated {
            self.census.observe("scoring_info", &snapshot.scoring.info);
            if let Some(row) = snapshot.player_scoring() {
                self.census.observe("player_row", row);
            }
        }
        let frame = out.frame.as_ref();
        let raw = frame.and(snapshot.player.as_ref().map(|p| &p.telemetry));
        if let Some(t) = raw {
            self.census.observe("telemetry", t);
        }
        let step = Step {
            snapshot,
            out: &out,
            capture_s,
            frame,
            raw,
            player_row: snapshot.player_scoring(),
            discontinuity,
        };
        self.channels.observe(&step);
        self.checks.observe(&step);
    }

    /// Names collected so far.
    pub fn names(&self) -> &NameCollector {
        self.session.names()
    }

    /// Builds the report.
    pub fn finish(
        self,
        file: String,
        file_bytes: Option<u64>,
        replay: ReplayStatus,
        compat: &CompatibilityReport,
        reader_stats: &ReaderStats,
    ) -> CaptureReport {
        let redactor = self.session.names().redactor();
        let timing = (self.snapshots > 0).then(|| self.timing.finish(reader_stats));
        let checks = self.checks.finish(&self.normalizer, timing.as_ref());
        CaptureReport {
            file: redactor.redact(&file),
            file_bytes,
            replay: redact_replay(replay, &redactor),
            session: self.session.finish(compat, &redactor),
            timing,
            channels: self.channels.finish(),
            census: self.census,
            checks,
            normalizer: self.normalizer.stats(),
        }
    }
}

fn redact_replay(mut replay: ReplayStatus, redactor: &Redactor) -> ReplayStatus {
    if let Some(stop) = &mut replay.stopped {
        stop.message = redactor.redact(&stop.message);
    }
    replay
}

/// `(frame index, kind)` of a capture error.
fn classify(e: &CaptureError) -> (Option<u64>, &'static str) {
    match e {
        CaptureError::Io(_) => (None, "IO"),
        CaptureError::BadMagic => (None, "BAD_MAGIC"),
        CaptureError::Version(_) => (None, "FORMAT_VERSION"),
        CaptureError::LayoutMismatch { .. } => (None, "LAYOUT_MISMATCH"),
        CaptureError::FingerprintMismatch { .. } => (None, "FINGERPRINT_MISMATCH"),
        CaptureError::Corrupt { frame, .. } => (Some(*frame), "CORRUPT"),
        CaptureError::Truncated { frame } => (Some(*frame), "TRUNCATED"),
        CaptureError::Block { frame, .. } => (Some(*frame), "BLOCK_LAYOUT"),
        CaptureError::Decode { frame, .. } => (Some(*frame), "DECODE"),
        CaptureError::Incompatible { frame, .. } => (Some(*frame), "INCOMPATIBLE"),
    }
}

fn stop_info(e: &CaptureError, snapshots_before: u64, last_capture_s: Option<f64>) -> StopInfo {
    let (frame_index, kind) = classify(e);
    StopInfo {
        frame_index,
        snapshots_before,
        last_capture_s,
        kind: kind.to_owned(),
        message: e.to_string(),
    }
}

/// Analyses a capture held by any reader (`file` is the name printed in the report).
pub fn analyze_reader<R: Read>(file: &str, file_bytes: Option<u64>, input: R) -> CaptureReport {
    let reader = match CaptureReader::new(input) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(component = COMPONENT, file, error = %e, "capture header rejected");
            let empty = CaptureAnalysis::new("");
            let replay = ReplayStatus {
                complete: false,
                frames_read: 0,
                snapshots: 0,
                stopped: Some(stop_info(&e, 0, None)),
            };
            return empty.finish(
                file.to_owned(),
                file_bytes,
                replay,
                &CompatTracker::default().report(),
                &ReaderStats::default(),
            );
        }
    };
    let mut replay = CaptureReplay::new(reader);
    let mut analysis = CaptureAnalysis::new(replay.meta());
    let mut last_ns: Option<u64> = None;
    let mut first_ns: Option<u64> = None;
    let stopped = loop {
        match replay.next_snapshot() {
            Ok(Some(snapshot)) => {
                first_ns.get_or_insert(snapshot.capture_mono_ns);
                last_ns = Some(snapshot.capture_mono_ns);
                analysis.observe(&snapshot, replay.stats());
            }
            Ok(None) => break None,
            Err(e) => {
                tracing::warn!(component = COMPONENT, file, error = %e, "replay stopped early");
                let last_s = first_ns
                    .zip(last_ns)
                    .map(|(a, b)| b.saturating_sub(a) as f64 / NS_PER_S);
                break Some(stop_info(&e, analysis.snapshots, last_s));
            }
        }
    };
    let stats = replay.stats().clone();
    let status = ReplayStatus {
        complete: stopped.is_none(),
        // Every frame record read is a snapshot, a byte-identical duplicate or a rejected frame.
        frames_read: stats.snapshots + stats.duplicate_frames + stats.rejected_frames,
        snapshots: analysis.snapshots,
        stopped,
    };
    let compat = replay.compatibility();
    analysis.finish(file.to_owned(), file_bytes, status, &compat, &stats)
}

/// Analyses a capture file. Never fails: a file that cannot be opened is reported as stopped at the header.
pub fn analyze_capture(path: &Path) -> CaptureReport {
    let name = path.file_name().map_or_else(
        || "capture".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    match File::open(path) {
        Ok(file) => {
            let bytes = file.metadata().ok().map(|m| m.len());
            analyze_reader(&name, bytes, BufReader::new(file))
        }
        Err(e) => analyze_reader(&name, None, OpenFailed(Some(e))),
    }
}

/// A reader that fails with the open error (so a missing file is reported like any other header error).
struct OpenFailed(Option<std::io::Error>);

impl Read for OpenFailed {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(self
            .0
            .take()
            .unwrap_or_else(|| std::io::Error::other("capture file could not be opened")))
    }
}

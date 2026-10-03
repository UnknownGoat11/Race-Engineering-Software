//! Turns copied frames into [`RawSnapshot`]s: duplicate detection, structural gate, decode,
//! freshness flags and content checks.
//!
//! [`FrameProcessor`] has no OS dependencies, so the live reader and capture replays share it and
//! produce identical snapshots for identical bytes.

use std::sync::Arc;

use crate::blocks::RawBlocks;
use crate::bytes::DecodeError;
use crate::compat::{
    CompatTracker, CompatibilityReport, ContentFacts, ContentInput, StructuralFacts,
};
use crate::raw_structs::{decode_generic, decode_paths};
use crate::snapshot::{
    PlayerIdentity, RawSnapshot, ScoringBlock, TelemetrySlots, decode_index_slot,
    decode_scoring_block, decode_telemetry_header, select_player,
};
use crate::state::FrameSource;
use crate::stats::ReaderStats;

/// A player ET step longer than this many `mDeltaTime`s means samples were skipped.
const SKIP_GAP_FACTOR: f64 = 1.5;
/// Gaps of more than this many samples are a discontinuity (pause, reload), not lost samples; they
/// are counted in [`ReaderStats::telemetry_gaps`] only.
const MAX_COUNTED_SKIPPED_SAMPLES: f64 = 1_000.0;
/// Plausible `mDeltaTime` values recorded in the histogram are below this (seconds).
const MAX_RECORDED_DELTA_TIME_S: f64 = 1.0;
/// Nanoseconds per second.
const NS_PER_S: f64 = 1e9;

/// Number of telemetry samples skipped between two consecutive player samples of the same car:
/// `round((et - prev_et) / dt) - 1` when the step exceeds [`SKIP_GAP_FACTOR`] × `dt`. `None` when no
/// gap; `Some(0)` for a gap too long to count (a discontinuity).
fn skipped_samples(prev_et: f64, et: f64, dt: f64) -> Option<u64> {
    if !(dt.is_finite() && dt > 0.0 && prev_et.is_finite() && et.is_finite()) {
        return None;
    }
    let gap = et - prev_et;
    if gap <= SKIP_GAP_FACTOR * dt {
        return None;
    }
    let missing = (gap / dt).round() - 1.0;
    if missing < 1.0 {
        return None;
    }
    // Bounded above, so the cast cannot saturate.
    Some(if missing <= MAX_COUNTED_SKIPPED_SAMPLES {
        missing as u64
    } else {
        0
    })
}

/// Metadata of one copied frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameMeta {
    /// Capture time (nanoseconds of the reader's [`crate::MonoClock`]).
    pub capture_mono_ns: u64,
    /// Connection generation.
    pub connection_id: u64,
    /// Synchronisation and consistency of the copy.
    pub source: FrameSource,
    /// `VirtualQuery` region size of the view (`None` for replays).
    pub mapping_size: Option<usize>,
}

/// What [`FrameProcessor::process`] made of a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum FrameOutcome {
    /// A new frame.
    Snapshot(Box<RawSnapshot>),
    /// Byte-identical to the previous accepted frame (not returned to consumers).
    Duplicate,
    /// A structural compatibility check failed; the frame was not decoded.
    Rejected,
}

/// Identity and freshness key of the player telemetry sample.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PlayerKey {
    slot: u8,
    id: i32,
    elapsed_time: f64,
}

impl PlayerKey {
    fn same_car(self, other: Self) -> bool {
        self.slot == other.slot && self.id == other.id
    }
}

/// Stateful frame processing for one reader (or one capture replay).
#[derive(Debug, Default)]
pub struct FrameProcessor {
    last: RawBlocks,
    has_last: bool,
    last_scoring: Option<Arc<ScoringBlock>>,
    last_player: Option<PlayerKey>,
    compat: CompatTracker,
    seq: u64,
}

impl FrameProcessor {
    /// A processor with no history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets the previous frame and the compatibility history (a new connection starts).
    /// Sequence numbers keep increasing.
    pub fn reset(&mut self) {
        self.has_last = false;
        self.last_scoring = None;
        self.last_player = None;
        self.compat = CompatTracker::new();
    }

    /// The blocks of the last accepted frame (the one behind the last returned snapshot).
    pub fn last_blocks(&self) -> Option<&RawBlocks> {
        self.has_last.then_some(&self.last)
    }

    /// Compatibility history of the current connection.
    pub fn compat(&self) -> &CompatTracker {
        &self.compat
    }

    /// Renders the compatibility report of the current connection.
    pub fn report(&self) -> CompatibilityReport {
        self.compat.report()
    }

    /// Records that the view could not be copied at all because it is too small.
    pub(crate) fn record_mapping_too_small(&mut self, mapping_size: usize) {
        self.compat
            .observe_structural(StructuralFacts::mapping_only(mapping_size));
    }

    /// Processes one copied frame. On success `work` receives the buffers of the previous frame (they
    /// are reused for the next copy) and the accepted frame stays available via
    /// [`FrameProcessor::last_blocks`].
    ///
    /// # Errors
    /// [`DecodeError`] if a copied block does not have its layout size (an internal invariant; the
    /// frame is dropped).
    pub fn process(
        &mut self,
        work: &mut RawBlocks,
        meta: &FrameMeta,
        stats: &mut ReaderStats,
    ) -> Result<FrameOutcome, DecodeError> {
        if self.has_last && work.same_content(&self.last) {
            stats.duplicate_frames += 1;
            return Ok(FrameOutcome::Duplicate);
        }
        if !self
            .compat
            .observe_structural(StructuralFacts::from_blocks(work, meta.mapping_size))
        {
            stats.rejected_frames += 1;
            return Ok(FrameOutcome::Rejected);
        }

        let generic = decode_generic(work.generic())?;
        let scoring_updated = !self.has_last || !work.same_scoring(&self.last);
        let scoring = match (&self.last_scoring, scoring_updated) {
            (Some(previous), false) => Arc::clone(previous),
            _ => Arc::new(decode_scoring_block(work)?),
        };
        let telemetry_header = decode_telemetry_header(work);
        let index_slot = decode_index_slot(work)?;
        let slots = TelemetrySlots::from_blocks(work);
        // The slot playerVehicleIdx names is only used as the player's after its mID is verified.
        let (player, player_identity) = select_player(index_slot.clone(), &scoring, &slots)?;
        let paths = work.paths().map(decode_paths).transpose()?;
        let scoring_stream = work.scoring_stream().map(<[u8]>::to_vec);

        // Freshness (A-LMU-031): a telemetry sample is new when the player ET changed, or the player
        // slot / car changed. Bit comparison so that NaN -> NaN counts as unchanged.
        let player_key = player.as_ref().map(|p| PlayerKey {
            slot: p.slot,
            id: p.telemetry.id,
            elapsed_time: p.telemetry.elapsed_time,
        });
        let telemetry_updated = match (self.last_player, player_key) {
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(a), Some(b)) => {
                !a.same_car(b) || a.elapsed_time.to_bits() != b.elapsed_time.to_bits()
            }
        };
        let et_regression = match (self.last_player, player_key) {
            (Some(a), Some(b)) if a.same_car(b) && b.elapsed_time < a.elapsed_time => {
                Some((a.elapsed_time, b.elapsed_time))
            }
            _ => None,
        };

        let verdict = self.compat.observe_content(ContentFacts::collect(
            work,
            &ContentInput {
                game_version: generic.game_version,
                events: &generic.events,
                scoring: &scoring,
                header: telemetry_header,
                index_slot: index_slot.as_ref(),
                et_regression,
                identity: player_identity,
            },
        ));
        if verdict.reject {
            // A-LMU-007 layout symptoms: nothing of this frame is delivered.
            stats.rejected_frames += 1;
            return Ok(FrameOutcome::Rejected);
        }

        if et_regression.is_some() {
            stats.telemetry_et_regressions += 1;
        }
        match player_identity {
            PlayerIdentity::FoundById { .. } => stats.player_found_by_id_frames += 1,
            PlayerIdentity::Unverified { .. } => stats.player_unverified_frames += 1,
            PlayerIdentity::NoVehicle | PlayerIdentity::Verified => {}
        }
        if telemetry_updated && let Some(p) = &player {
            let dt = p.telemetry.delta_time;
            if dt.is_finite() && dt > 0.0 && dt < MAX_RECORDED_DELTA_TIME_S {
                // Bounded by MAX_RECORDED_DELTA_TIME_S, so the cast cannot saturate.
                stats.delta_time.record((dt * NS_PER_S) as u64);
            }
            if let (Some(a), Some(b)) = (self.last_player, player_key)
                && a.same_car(b)
                && let Some(skipped) = skipped_samples(a.elapsed_time, b.elapsed_time, dt)
            {
                stats.telemetry_gaps += 1;
                stats.telemetry_samples_skipped += skipped;
            }
        }

        self.seq += 1;
        let now = meta.capture_mono_ns;
        stats.snapshots += 1;
        stats.snapshot_rate.record(now);
        stats.last_snapshot_ns = Some(now);
        stats.record_events(&generic.events);
        if telemetry_updated {
            stats.telemetry_updates += 1;
            stats.telemetry_rate.record(now);
            stats.last_telemetry_update_ns = Some(now);
        }
        if scoring_updated {
            stats.scoring_updates += 1;
            stats.scoring_rate.record(now);
            stats.last_scoring_update_ns = Some(now);
        }
        if !telemetry_updated && !scoring_updated {
            stats.same_et_frames += 1;
        }

        let snapshot = RawSnapshot {
            seq: self.seq,
            capture_mono_ns: now,
            connection_id: meta.connection_id,
            source: meta.source,
            telemetry_updated,
            scoring_updated,
            support: verdict.support,
            version_verified: verdict.version_verified,
            generic,
            scoring: Arc::clone(&scoring),
            telemetry_header,
            player,
            player_identity,
            vehicle_telemetry: Arc::new(slots),
            paths,
            scoring_stream,
        };
        std::mem::swap(work, &mut self.last);
        self.has_last = true;
        self.last_scoring = Some(scoring);
        self.last_player = player_key;
        Ok(FrameOutcome::Snapshot(Box::new(snapshot)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{CopyPlan, copy_frame};
    use crate::compat::{CheckId, Outcome, SupportLevel};
    use crate::offsets::{scoring_info, telem_info};
    use crate::state::Consistency;
    use crate::testutil::LayoutBuilder;

    const META: FrameMeta = FrameMeta {
        capture_mono_ns: 1_000,
        connection_id: 1,
        source: FrameSource {
            sync: None,
            consistency: Consistency::Replayed { locked: true },
        },
        mapping_size: Some(crate::offsets::layout::SIZE),
    };

    fn run(p: &mut FrameProcessor, b: &LayoutBuilder, stats: &mut ReaderStats) -> FrameOutcome {
        let mut work = RawBlocks::new();
        copy_frame(b.bytes(), CopyPlan::default(), &mut work).expect("copy");
        p.process(&mut work, &META, stats).expect("process")
    }

    fn snapshot(outcome: FrameOutcome) -> RawSnapshot {
        match outcome {
            FrameOutcome::Snapshot(s) => *s,
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    fn outcome_of(report: &CompatibilityReport, id: CheckId) -> Option<Outcome> {
        report
            .checks
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.outcome)
    }

    #[test]
    fn first_frame_is_fresh_and_identical_frames_are_duplicates() {
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        let first = snapshot(run(&mut p, &b, &mut stats));
        assert!(first.telemetry_updated && first.scoring_updated);
        assert_eq!(first.seq, 1);
        assert_eq!(run(&mut p, &b, &mut stats), FrameOutcome::Duplicate);
        assert_eq!(stats.duplicate_frames, 1);

        // New telemetry sample only: scoring is shared, not re-decoded.
        b.player_elapsed_time(0, 120.01);
        let second = snapshot(run(&mut p, &b, &mut stats));
        assert!(second.telemetry_updated);
        assert!(!second.scoring_updated);
        assert!(Arc::ptr_eq(&first.scoring, &second.scoring));
        assert_eq!(second.seq, 2);

        // Scoring update only.
        b.scoring_et(120.2);
        let third = snapshot(run(&mut p, &b, &mut stats));
        assert!(!third.telemetry_updated && third.scoring_updated);
        assert!(!Arc::ptr_eq(&second.scoring, &third.scoring));
        assert_eq!(stats.telemetry_updates, 2);
        assert_eq!(stats.scoring_updates, 2);
        assert_eq!(
            p.last_blocks().and_then(RawBlocks::scoring_current_et),
            Some(120.2)
        );
    }

    #[test]
    fn generic_only_change_is_counted_as_same_et() {
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        snapshot(run(&mut p, &b, &mut stats));
        b.event(0, crate::offsets::sme::FFB);
        let s = snapshot(run(&mut p, &b, &mut stats));
        assert!(!s.telemetry_updated && !s.scoring_updated);
        assert_eq!(stats.same_et_frames, 1);
    }

    #[test]
    fn unverified_game_version_is_reported_separately_and_does_not_lower_support() {
        // A future version nobody has verified yet (14200 is verified, see the next test).
        const UNVERIFIED_VERSION: i32 = 14_300;
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        b.game_version(UNVERIFIED_VERSION);
        let s = snapshot(run(&mut p, &b, &mut stats));
        let report = p.report();
        let warnings: Vec<CheckId> = report.warnings().map(|c| c.id).collect();
        assert_eq!(warnings, vec![CheckId::GameVersion], "{report:#?}");
        assert_eq!(report.scoring, Some(SupportLevel::Supported));
        assert_eq!(report.telemetry, Some(SupportLevel::Supported));
        assert_eq!(report.generic, Some(SupportLevel::Supported));
        assert_eq!(report.overall, Some(SupportLevel::Supported));
        assert_eq!(report.version_verified, Some(false));
        // A clean frame is SUPPORTED; a frame with a content warning is not (the level carries
        // information again).
        assert_eq!(s.support, SupportLevel::Supported);
        assert!(!s.version_verified);
        assert_eq!(s.player_identity, PlayerIdentity::Verified);
        assert_eq!(s.vehicle_telemetry.len(), 2, "opponent slots are copied");
        assert_eq!(report.game_version, Some(UNVERIFIED_VERSION));
        assert!(report.is_decodable());
    }

    #[test]
    fn gate1_verified_version_and_empty_event_slots_raise_no_warning() {
        // LMU 1.4.2.0 (gameVersion 14200) was verified at Gate 1; its unused event slots read SME_MAX.
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        b.event(0, crate::offsets::sme::UPDATE_TELEMETRY);
        b.event(1, crate::offsets::sme::FFB);
        for slot in 2..crate::offsets::EVENT_SLOTS {
            b.event(slot, crate::offsets::sme::MAX);
        }
        let s = snapshot(run(&mut p, &b, &mut stats));
        let report = p.report();
        assert_eq!(report.warnings().count(), 0, "{report:#?}");
        assert_eq!(report.version_verified, Some(true));
        assert!(s.version_verified);
        assert_eq!(s.support, SupportLevel::Supported);
        assert_eq!(stats.unknown_event_codes, 0);
        assert_eq!(stats.empty_event_slots, 14);
    }

    #[test]
    fn combined_layout_symptoms_reject_the_frame() {
        // Player slot with garbage where ET/dt, names and mID live: a shifted telemInfo layout.
        let mut b = LayoutBuilder::on_track_frame();
        let slot = LayoutBuilder::telem_base(0);
        b.set(slot, telem_info::DELTA_TIME, -3.0e200f64);
        b.set(slot, telem_info::ID, 0x5a5a_5a5ai32);
        b.set_raw(slot, telem_info::VEHICLE_NAME, &[0x01, 0x02, 0x03, 0]);
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        assert_eq!(run(&mut p, &b, &mut stats), FrameOutcome::Rejected);
        assert_eq!(stats.snapshots, 0, "nothing of the frame is delivered");
        assert_eq!(stats.rejected_frames, 1);
        let report = p.report();
        assert_eq!(
            outcome_of(&report, CheckId::TelemetryLayout),
            Some(Outcome::Fail)
        );
        assert_eq!(report.telemetry, Some(SupportLevel::Unsupported));
        assert!(!report.is_decodable());
        assert!(p.last_blocks().is_none());

        // One symptom alone only warns and the frame is delivered.
        let mut b = LayoutBuilder::on_track_frame();
        b.set(LayoutBuilder::telem_base(0), telem_info::DELTA_TIME, 0.5f64);
        let mut p = FrameProcessor::new();
        snapshot(run(&mut p, &b, &mut stats));
        assert_eq!(
            outcome_of(&p.report(), CheckId::TelemetryLayout),
            Some(Outcome::Pass)
        );
    }

    #[test]
    fn unreadable_scoring_names_everywhere_reject_the_frame() {
        let mut b = LayoutBuilder::on_track_frame();
        let garbage = [0x07u8; 8];
        b.set_raw(
            LayoutBuilder::scoring_base(),
            scoring_info::TRACK_NAME,
            &garbage,
        );
        for row in 0..2 {
            b.set_raw(
                LayoutBuilder::vehicle_base(row),
                crate::offsets::veh_scoring::DRIVER_NAME,
                &garbage,
            );
        }
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        assert_eq!(run(&mut p, &b, &mut stats), FrameOutcome::Rejected);
        assert_eq!(
            outcome_of(&p.report(), CheckId::ScoringLayout),
            Some(Outcome::Fail)
        );
    }

    #[test]
    fn skipped_samples_and_delta_time_are_measured() {
        assert_eq!(skipped_samples(1.0, 1.01, 0.01), None);
        assert_eq!(skipped_samples(1.0, 1.03, 0.01), Some(2));
        assert_eq!(skipped_samples(1.0, 100.0, 0.01), Some(0), "discontinuity");
        assert_eq!(skipped_samples(1.0, 1.05, f64::NAN), None);
        assert_eq!(skipped_samples(1.0, 1.05, 0.0), None);

        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        snapshot(run(&mut p, &b, &mut stats));
        b.player_elapsed_time(0, 120.05); // dt is 0.01 s: 4 samples were never seen
        snapshot(run(&mut p, &b, &mut stats));
        assert_eq!(stats.telemetry_samples_skipped, 4);
        assert_eq!(stats.telemetry_gaps, 1);
        assert_eq!(stats.delta_time.count(), 2);
        assert_eq!(stats.delta_time.max_ns(), Some(10_000_000));
    }

    #[test]
    fn non_zero_padding_rejects_the_frame_without_decoding() {
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        b.scoring_padding([1, 0, 0, 0]);
        assert_eq!(run(&mut p, &b, &mut stats), FrameOutcome::Rejected);
        assert_eq!(stats.rejected_frames, 1);
        assert_eq!(stats.snapshots, 0);
        let report = p.report();
        assert_eq!(
            outcome_of(&report, CheckId::ScoringPadding),
            Some(Outcome::Fail)
        );
        assert_eq!(report.scoring, Some(SupportLevel::Unsupported));
        assert_eq!(report.telemetry, Some(SupportLevel::Unsupported));
        // The generic block lies before the hole: only the sections behind it are affected.
        assert_eq!(report.generic, Some(SupportLevel::Supported));
        assert_eq!(report.overall, Some(SupportLevel::Unsupported));
        assert!(!report.is_decodable());
        let message = report.incompatibility_message();
        assert!(
            message.contains("padding bytes at offset 2180: detected [01, 00, 00, 00]"),
            "{message}"
        );
        assert!(
            p.last_blocks().is_none(),
            "a rejected frame never becomes the reference frame"
        );
    }

    #[test]
    fn every_structural_failure_rejects() {
        type Mutate = fn(&mut LayoutBuilder);
        let cases: [(CheckId, Mutate); 5] = [
            (CheckId::ScoringStreamSize, |b| {
                b.scoring_stream_size(65_537);
            }),
            (CheckId::NumVehicles, |b| {
                b.num_vehicles(105);
            }),
            (CheckId::NumVehicles, |b| {
                b.num_vehicles(-1);
            }),
            (CheckId::ActiveVehicles, |b| {
                b.telemetry_header(105, 0, 1);
            }),
            (CheckId::PlayerHasVehicle, |b| {
                b.telemetry_header(2, 0, 2);
            }),
        ];
        for (id, mutate) in cases {
            let mut p = FrameProcessor::new();
            let mut stats = ReaderStats::default();
            let mut b = LayoutBuilder::on_track_frame();
            mutate(&mut b);
            assert_eq!(
                run(&mut p, &b, &mut stats),
                FrameOutcome::Rejected,
                "{id:?}"
            );
            assert_eq!(outcome_of(&p.report(), id), Some(Outcome::Fail), "{id:?}");
        }
    }

    #[test]
    fn stream_size_at_capacity_is_accepted() {
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        b.scoring_stream_size(65_536);
        snapshot(run(&mut p, &b, &mut stats));
    }

    #[test]
    fn content_problems_only_warn_and_are_counted() {
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        let slot = LayoutBuilder::telem_base(0);
        b.set(slot, telem_info::DELTA_TIME, 0.5f64);
        b.set(slot, telem_info::ABS_ACTIVE, 7u8);
        b.set(
            LayoutBuilder::scoring_base(),
            scoring_info::CURRENT_ET,
            f64::NAN,
        );
        b.event(3, 42);
        let s = snapshot(run(&mut p, &b, &mut stats));
        assert_eq!(s.support, SupportLevel::SupportedWithWarnings);
        assert!(
            s.scoring.info.current_et.is_nan(),
            "NaN passes through to the consumer"
        );
        let report = p.report();
        for id in [
            CheckId::TelemetryEt,
            CheckId::TelemetryBools,
            CheckId::ScoringEt,
            CheckId::EventCodes,
        ] {
            assert_eq!(outcome_of(&report, id), Some(Outcome::Warn), "{id:?}");
        }
        let bools = report
            .checks
            .iter()
            .find(|c| c.id == CheckId::TelemetryBools)
            .expect("check");
        assert!(
            bools.detected.contains("mABSActive = 7"),
            "{}",
            bools.detected
        );
        assert_eq!(stats.unknown_event_codes, 1);
    }

    #[test]
    fn player_index_and_identity_problems_warn() {
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        b.telemetry_header(2, 2, 1); // index == activeVehicles: no player slot is copied
        let s = snapshot(run(&mut p, &b, &mut stats));
        assert!(s.player.is_none());
        assert_eq!(
            outcome_of(&p.report(), CheckId::PlayerIndex),
            Some(Outcome::Warn)
        );

        let mut b = LayoutBuilder::on_track_frame();
        b.set(LayoutBuilder::telem_base(0), telem_info::ID, 9i32);
        let mut p = FrameProcessor::new();
        let s = snapshot(run(&mut p, &b, &mut stats));
        assert_eq!(
            outcome_of(&p.report(), CheckId::PlayerScoringRow),
            Some(Outcome::Warn)
        );
        // Research §11.4: the player is UNAVAILABLE, never the other car's data.
        assert!(s.player.is_none());
        assert!(!s.telemetry_updated);
        assert!(matches!(
            s.player_identity,
            PlayerIdentity::Unverified { .. }
        ));
        assert_eq!(stats.player_unverified_frames, 1);

        // The index points at the rival's slot but the player's mID is in slot 0: found by mID.
        let mut b = LayoutBuilder::on_track_frame();
        b.telemetry_header(2, 1, 1);
        let mut p = FrameProcessor::new();
        let s = snapshot(run(&mut p, &b, &mut stats));
        assert_eq!(
            s.player.as_ref().map(|p| (p.slot, p.telemetry.id)),
            Some((0, 7))
        );
        assert_eq!(
            s.player_identity,
            PlayerIdentity::FoundById { index_slot: 1 }
        );
        assert_eq!(stats.player_found_by_id_frames, 1);
    }

    #[test]
    fn et_regression_is_detected_and_counted() {
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::on_track_frame();
        snapshot(run(&mut p, &b, &mut stats));
        b.player_elapsed_time(0, 3.0);
        let s = snapshot(run(&mut p, &b, &mut stats));
        assert!(s.telemetry_updated);
        assert_eq!(stats.telemetry_et_regressions, 1);
        let report = p.report();
        assert_eq!(
            outcome_of(&report, CheckId::TelemetryEtMonotonic),
            Some(Outcome::Warn)
        );
        b.player_elapsed_time(0, 3.01);
        snapshot(run(&mut p, &b, &mut stats));
        let report = p.report();
        let check = report
            .checks
            .iter()
            .find(|c| c.id == CheckId::TelemetryEtMonotonic)
            .expect("check");
        assert_eq!(check.outcome, Some(Outcome::Pass));
        assert_eq!(check.warn_frames, 1);
        assert!(
            check
                .last_problem
                .as_deref()
                .is_some_and(|t| t.contains("from 120 to 3"))
        );
    }

    #[test]
    fn menus_without_a_player_vehicle_are_supported() {
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut b = LayoutBuilder::new(); // fresh, zero-filled mapping
        b.game_version(14_200);
        let s = snapshot(run(&mut p, &b, &mut stats));
        assert!(s.player.is_none());
        assert!(!s.telemetry_updated);
        assert!(s.scoring.vehicles.is_empty());
        let report = p.report();
        assert_eq!(
            report.telemetry,
            Some(SupportLevel::Supported),
            "{report:#?}"
        );
        assert!(report.is_decodable());
    }

    #[test]
    fn reset_forgets_the_reference_frame_but_keeps_sequence_numbers() {
        let mut p = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let b = LayoutBuilder::on_track_frame();
        snapshot(run(&mut p, &b, &mut stats));
        p.reset();
        let again = snapshot(run(&mut p, &b, &mut stats));
        assert_eq!(again.seq, 2);
        assert!(
            again.telemetry_updated,
            "after a reconnect the first frame is fresh"
        );
        assert_eq!(p.compat().frames_checked(), 1);
    }
}

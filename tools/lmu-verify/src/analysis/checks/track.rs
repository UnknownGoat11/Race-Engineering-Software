//! Track-position and timing checks: scoring lap distance (monotonic, wraps at the line) and sector
//! semantics with sector / lap times (A-LMU-026, A-LMU-043).

use lmu_reader::split_current_sector;

use super::{CheckResult, Outcome, Verdict, fmt_opt, pct};
use crate::analysis::Step;
use crate::stats::{Agreement, Summary};

// ----------------------------------------------------------------------------------------------------------
// Lap distance
// ----------------------------------------------------------------------------------------------------------

/// A within-lap step back larger than this is a regression (m); smaller ones are jitter.
const LAP_DIST_BACK_TOLERANCE_M: f64 = 5.0;
/// A wrap goes from above this share of the track length ...
const WRAP_FROM_SHARE: f64 = 0.9;
/// ... to below this share.
const WRAP_TO_SHARE: f64 = 0.1;
/// A wrap and a lap increment within this many scoring updates belong together.
const WRAP_PAIR_UPDATES: u64 = 2;
/// Lap distance outside `-margin..=length + margin` is out of range on a counted lap (m). Before the first
/// line crossing (lap counter 0) and in the pit area the car can be behind the line: LMU reports down to
/// −1 track length there (Gate 1 race: −1,586.8 m in the garage, −1,023.4 m on the grid; A-LMU-070).
const LAP_DIST_MARGIN_M: f64 = 50.0;
/// Backward share accepted (fraction of scoring updates).
const BACKWARD_PASS_SHARE: f64 = 0.005;
/// Backward share that fails.
const BACKWARD_FAIL_SHARE: f64 = 0.05;
/// Paired wraps needed for a PASS.
const PAIRED_PASS_SHARE: f64 = 0.9;
/// Paired wraps below this fail.
const PAIRED_FAIL_SHARE: f64 = 0.5;
/// Problem examples listed.
const LISTED_EXAMPLES: usize = 8;

/// Scoring `mLapDist` of the player: monotonic within a lap, wrapping at the line with the lap counter.
#[derive(Debug, Clone, Default)]
pub struct LapDistCheck {
    prev: Option<(i16, f64)>,
    updates: u64,
    backward: u64,
    out_of_range: u64,
    wraps: u64,
    increments: u64,
    pit_increments: u64,
    paired: u64,
    /// Updates behind the line (negative distance) before the first crossing or in the pit area.
    behind_line: u64,
    /// Start re-bases: the distance re-measured across the line before the first crossing (Gate 1 race:
    /// −1,023 → +4,797 m at the formation start, +5,726 → −87 m at the green flag), not line crossings.
    start_rebases: u64,
    since_wrap: Option<u64>,
    since_increment: Option<u64>,
    examples: Vec<String>,
    track_length: Summary,
}

impl LapDistCheck {
    pub(super) fn observe(&mut self, step: &Step<'_>) {
        if step.discontinuity {
            self.prev = None;
            self.since_wrap = None;
            self.since_increment = None;
        }
        if !step.snapshot.scoring_updated {
            return;
        }
        let Some(row) = step.player_row else {
            return;
        };
        let length = step.snapshot.scoring.info.lap_dist;
        let (laps, d) = (row.total_laps, row.lap_dist);
        if !(d.is_finite() && length.is_finite() && length > 0.0) {
            return;
        }
        self.track_length.push(length);
        self.updates += 1;
        self.since_wrap = self.since_wrap.map(|n| n + 1);
        self.since_increment = self.since_increment.map(|n| n + 1);
        let in_pit_area = row.in_pits || row.in_garage_stall;
        let before_first_crossing = laps == 0;
        let behind_line_allowed = before_first_crossing || in_pit_area;
        if d < -length - LAP_DIST_MARGIN_M
            || d > length + LAP_DIST_MARGIN_M
            || (d < -LAP_DIST_MARGIN_M && !behind_line_allowed)
        {
            self.out_of_range += 1;
        } else if d < -LAP_DIST_MARGIN_M {
            self.behind_line += 1;
        }
        if let Some((prev_laps, prev_d)) = self.prev {
            let increment = laps == prev_laps.saturating_add(1);
            let crossing = prev_d > length * WRAP_FROM_SHARE && d < length * WRAP_TO_SHARE;
            // Before the first crossing LMU re-measures the distance across the line (to a negative value at
            // the green flag, or by +1 lap at the formation start) without counting a lap.
            let rebase = before_first_crossing
                && laps == prev_laps
                && ((crossing && d < 0.0)
                    || (prev_d < 0.0 && d - prev_d > length * WRAP_FROM_SHARE));
            if rebase {
                self.start_rebases += 1;
                if self.examples.len() < LISTED_EXAMPLES {
                    self.examples.push(format!(
                        "start re-base before the first crossing: {prev_d:.1} → {d:.1} m at capture {:.1} s",
                        step.capture_s
                    ));
                }
            }
            let wrap = crossing && !rebase;
            if wrap || increment || laps != prev_laps {
                tracing::debug!(
                    component = crate::COMPONENT,
                    capture_s = step.capture_s,
                    prev_laps,
                    laps,
                    prev_d,
                    d,
                    in_pits = row.in_pits,
                    wrap,
                    "lap distance event"
                );
            }
            if wrap {
                self.wraps += 1;
                if self.since_increment.is_some_and(|n| n <= WRAP_PAIR_UPDATES) || increment {
                    self.paired += 1;
                    self.since_increment = None;
                } else {
                    self.since_wrap = Some(0);
                }
            }
            if increment && !wrap && in_pit_area {
                // A lap counted on arrival in the pits / garage (e.g. a return to the garage from the
                // track): the distance jumps to the pit area instead of wrapping at the line.
                self.pit_increments += 1;
                if self.examples.len() < LISTED_EXAMPLES {
                    self.examples.push(format!(
                        "lap {prev_laps} → {laps} counted in the pits/garage: {prev_d:.1} → {d:.1} m at capture {:.1} s",
                        step.capture_s
                    ));
                }
            } else if increment {
                self.increments += 1;
                if !wrap {
                    if self.since_wrap.is_some_and(|n| n <= WRAP_PAIR_UPDATES) {
                        self.paired += 1;
                        self.since_wrap = None;
                    } else {
                        self.since_increment = Some(0);
                    }
                }
            }
            if !wrap
                && !rebase
                && laps == prev_laps
                && d < prev_d - LAP_DIST_BACK_TOLERANCE_M
                && !in_pit_area
            {
                self.backward += 1;
                if self.examples.len() < LISTED_EXAMPLES {
                    self.examples.push(format!(
                        "step back in lap {laps}: {prev_d:.1} → {d:.1} m at capture {:.1} s",
                        step.capture_s
                    ));
                }
            }
        }
        self.prev = Some((laps, d));
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "lap_distance",
            "Lap distance monotonic within a lap, wrapping at the line",
            &[],
            &["track.lap_dist_m", "track.length_m", "race.laps_completed"],
        );
        let backward_share = (self.updates > 0).then(|| self.backward as f64 / self.updates as f64);
        let events = self.wraps.max(self.increments);
        let paired_share = (events > 0).then(|| self.paired as f64 / events as f64);
        o.metric("scoring_updates", self.updates as f64)
            .metric("backward_steps", self.backward as f64)
            .metric("wraps", self.wraps as f64)
            .metric("lap_increments", self.increments as f64)
            .metric("lap_increments_in_pits", self.pit_increments as f64)
            .metric("paired", self.paired as f64)
            .metric("out_of_range", self.out_of_range as f64)
            .metric("behind_line_updates", self.behind_line as f64)
            .metric("start_rebases", self.start_rebases as f64)
            .metric_opt("track_length_m", self.track_length.median());
        o.line(format!(
            "player scoring updates {}; track length {} m; within-lap steps back > {LAP_DIST_BACK_TOLERANCE_M} m outside the pits: {} ({})",
            self.updates,
            fmt_opt(self.track_length.median(), 1),
            self.backward,
            pct(backward_share)
        ));
        o.line(format!(
            "wraps (> {WRAP_FROM_SHARE} L → < {WRAP_TO_SHARE} L) {}, lap-counter increments on track {}, paired within {WRAP_PAIR_UPDATES} updates {}; increments counted in the pits/garage without a wrap {}; out of range {}",
            self.wraps, self.increments, self.paired, self.pit_increments, self.out_of_range
        ));
        o.line(format!(
            "behind the line (< −{LAP_DIST_MARGIN_M} m) before the first crossing or in the pit area: {} updates; start re-bases (not line crossings): {}",
            self.behind_line, self.start_rebases
        ));
        for e in &self.examples {
            o.line(e.clone());
        }
        let summary = format!(
            "{} wraps / {} on-track lap increments ({} paired), {} counted in the pits, {} steps back, {} start re-base(s), {} out of range",
            self.wraps,
            self.increments,
            self.paired,
            self.pit_increments,
            self.backward,
            self.start_rebases,
            self.out_of_range
        );
        let fail = backward_share.is_some_and(|s| s > BACKWARD_FAIL_SHARE)
            || paired_share.is_some_and(|s| s < PAIRED_FAIL_SHARE && events >= 2)
            || self.out_of_range > 0;
        if fail {
            o.verdict(Verdict::Fail, summary)
        } else if self.increments > 0
            && backward_share.is_some_and(|s| s <= BACKWARD_PASS_SHARE)
            && paired_share.is_some_and(|s| s >= PAIRED_PASS_SHARE)
        {
            o.verdict(Verdict::Pass, summary)
        } else {
            o.verdict(Verdict::Inconclusive, summary)
        }
    }
}

// ----------------------------------------------------------------------------------------------------------
// Sectors and lap times
// ----------------------------------------------------------------------------------------------------------

/// Scoring updates within this capture time of a telemetry sector change are not compared (s).
const SECTOR_GUARD_S: f64 = 0.5;
/// A telemetry crossing waits at most this long for the scoring time (capture s).
const PENDING_MAX_S: f64 = 3.0;
/// Sector time agreement (s): telemetry crossing ET is quantised to the 10 ms sample step.
const SECTOR_TIME_TOLERANCE_S: f64 = 0.1;
/// Lap time agreement (s).
const LAP_TIME_TOLERANCE_S: f64 = 0.05;
/// Sector mapping agreement for a PASS.
const MAPPING_PASS_SHARE: f64 = 0.95;
/// Sector mapping agreement below this fails.
const MAPPING_FAIL_SHARE: f64 = 0.5;
/// Scoring updates needed for a mapping verdict.
const MIN_MAPPING_UPDATES: u64 = 50;
/// Time agreement for a PASS.
const TIME_PASS_SHARE: f64 = 0.9;
/// Number of sectors.
const SECTORS: i32 = 3;
/// Offset of scoring `mSector` relative to the zero-based S1 index: S1 = (mSector + 2) mod 3 (A-LMU-026).
const SCORING_SECTOR_OFFSET: i32 = 2;

/// A telemetry-side crossing waiting for its scoring time.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pending {
    time_s: f64,
    at_capture_s: f64,
}

/// Scoring sector semantics and sector / lap times vs the telemetry crossings.
#[derive(Debug, Clone, Default)]
pub struct SectorLapCheck {
    tel_sector: Option<u32>,
    tel_lap: Option<(i32, f64)>,
    last_sector_change_s: Option<f64>,
    mapping: Agreement,
    direct: Agreement,
    pending_s1: Option<Pending>,
    pending_s12: Option<Pending>,
    pending_lap: Option<Pending>,
    prev_cur_s1: Option<f64>,
    prev_cur_s12: Option<f64>,
    prev_last_lap: Option<f64>,
    s1_diff: Summary,
    s12_diff: Summary,
    lap_diff: Summary,
    s1_match: Agreement,
    s12_match: Agreement,
    lap_match: Agreement,
    lap_sentinels: u64,
}

impl SectorLapCheck {
    fn telemetry(&mut self, step: &Step<'_>) {
        let Some(t) = step.raw else {
            return;
        };
        let et = t.elapsed_time;
        let (sector, _) = split_current_sector(t.current_sector);
        if let Some(prev) = self.tel_sector
            && prev != sector
        {
            self.last_sector_change_s = Some(step.capture_s);
            let elapsed = et - t.lap_start_et;
            let pending = (t.lap_start_et >= 0.0 && elapsed > 0.0).then_some(Pending {
                time_s: elapsed,
                at_capture_s: step.capture_s,
            });
            match (prev, sector) {
                (0, 1) => self.pending_s1 = pending,
                (1, 2) => self.pending_s12 = pending,
                _ => {}
            }
        }
        self.tel_sector = Some(sector);
        if let Some((lap, start)) = self.tel_lap
            // `checked_add`: the lap number is raw capture data and may be garbage (i32::MAX).
            && lap.checked_add(1) == Some(t.lap_number)
            && start >= 0.0
            && t.lap_start_et > start
        {
            self.pending_lap = Some(Pending {
                time_s: t.lap_start_et - start,
                at_capture_s: step.capture_s,
            });
        }
        self.tel_lap = Some((t.lap_number, t.lap_start_et));
    }

    /// Compares a scoring time that just changed with a pending telemetry crossing.
    fn compare(
        pending: &mut Option<Pending>,
        value: f64,
        capture_s: f64,
        tolerance: f64,
        diff: &mut Summary,
        matches: &mut Agreement,
    ) {
        if let Some(p) = pending.take()
            && capture_s - p.at_capture_s <= PENDING_MAX_S
        {
            let d = value - p.time_s;
            diff.push(d);
            matches.push(d.abs() <= tolerance);
        }
    }

    fn scoring(&mut self, step: &Step<'_>) {
        if !step.snapshot.scoring_updated {
            return;
        }
        let Some(row) = step.player_row else {
            return;
        };
        let raw = i32::from(row.sector);
        let guarded = self
            .last_sector_change_s
            .is_some_and(|c| step.capture_s - c < SECTOR_GUARD_S);
        if let (Some(tel), false, true) = (self.tel_sector, guarded, (0..SECTORS).contains(&raw)) {
            let mapped = (raw + SCORING_SECTOR_OFFSET) % SECTORS;
            self.mapping.push(i64::from(mapped) == i64::from(tel));
            self.direct.push(i64::from(raw) == i64::from(tel));
        }
        let changed = |prev: Option<f64>, now: f64| now > 0.0 && prev != Some(now);
        if changed(self.prev_cur_s1, row.cur_sector1) {
            Self::compare(
                &mut self.pending_s1,
                row.cur_sector1,
                step.capture_s,
                SECTOR_TIME_TOLERANCE_S,
                &mut self.s1_diff,
                &mut self.s1_match,
            );
        }
        if changed(self.prev_cur_s12, row.cur_sector2) {
            Self::compare(
                &mut self.pending_s12,
                row.cur_sector2,
                step.capture_s,
                SECTOR_TIME_TOLERANCE_S,
                &mut self.s12_diff,
                &mut self.s12_match,
            );
        }
        if let Some(prev) = self.prev_last_lap
            && row.last_lap_time != prev
        {
            if row.last_lap_time <= 0.0 {
                // "No time" sentinel (A-LMU-043) right after a lap: nothing to compare.
                self.lap_sentinels += 1;
                self.pending_lap = None;
            } else {
                Self::compare(
                    &mut self.pending_lap,
                    row.last_lap_time,
                    step.capture_s,
                    LAP_TIME_TOLERANCE_S,
                    &mut self.lap_diff,
                    &mut self.lap_match,
                );
            }
        }
        self.prev_cur_s1 = Some(row.cur_sector1);
        self.prev_cur_s12 = Some(row.cur_sector2);
        self.prev_last_lap = Some(row.last_lap_time);
    }

    pub(super) fn observe(&mut self, step: &Step<'_>) {
        if step.discontinuity {
            *self = Self {
                mapping: self.mapping,
                direct: self.direct,
                s1_diff: std::mem::take(&mut self.s1_diff),
                s12_diff: std::mem::take(&mut self.s12_diff),
                lap_diff: std::mem::take(&mut self.lap_diff),
                s1_match: self.s1_match,
                s12_match: self.s12_match,
                lap_match: self.lap_match,
                lap_sentinels: self.lap_sentinels,
                ..Self::default()
            };
        }
        self.telemetry(step);
        self.scoring(step);
    }

    pub(super) fn finish(self) -> CheckResult {
        let mut o = Outcome::new(
            "sectors_and_laps",
            "Sector semantics and sector / lap times vs telemetry crossings",
            &["A-LMU-026", "A-LMU-043"],
            &[
                "track.sector_index",
                "track.sector_index_scoring",
                "timing.cur_sector1_s",
                "timing.cur_sector12_s",
                "timing.last_lap_s",
                "lap.start_et_s",
            ],
        );
        let mapping = self.mapping.fraction();
        o.metric_opt("mapping_agreement", mapping)
            .metric_opt("direct_agreement", self.direct.fraction())
            .metric("mapping_updates", self.mapping.total as f64)
            .metric_opt("s1_match", self.s1_match.fraction())
            .metric_opt("s12_match", self.s12_match.fraction())
            .metric_opt("lap_match", self.lap_match.fraction())
            .metric("laps_compared", self.lap_match.total as f64)
            .metric("lap_time_sentinels", self.lap_sentinels as f64);
        o.line(format!(
            "scoring mSector → (mSector + 2) mod 3 equals the telemetry sector index (mCurrentSector & 0x7FFFFFFF) in {} of {} scoring updates (direct equality: {}); updates within {SECTOR_GUARD_S} s of a crossing skipped",
            pct(mapping),
            self.mapping.total,
            pct(self.direct.fraction())
        ));
        let line = |name: &str, m: &Agreement, d: &Summary, tol: f64| {
            format!(
                "{name}: {} of {} within ±{tol} s of the telemetry crossing (scoring − telemetry median {} s, max |…| {} s)",
                pct(m.fraction()),
                m.total,
                fmt_opt(d.median(), 3),
                fmt_opt(d.min().zip(d.max()).map(|(a, b)| a.abs().max(b.abs())), 3)
            )
        };
        o.line(line(
            "sector 1 time (mCurSector1)",
            &self.s1_match,
            &self.s1_diff,
            SECTOR_TIME_TOLERANCE_S,
        ));
        o.line(line(
            "sector 1+2 time (mCurSector2)",
            &self.s12_match,
            &self.s12_diff,
            SECTOR_TIME_TOLERANCE_S,
        ));
        o.line(line(
            "lap time (mLastLapTime vs Δ mLapStartET)",
            &self.lap_match,
            &self.lap_diff,
            LAP_TIME_TOLERANCE_S,
        ));
        o.line(format!(
            "laps reported with the 'no time' sentinel (≤ 0): {}",
            self.lap_sentinels
        ));
        let summary = format!(
            "(mSector + 2) mod 3 matches in {}; lap times match {}; S1 times match {}",
            pct(mapping),
            pct(self.lap_match.fraction()),
            pct(self.s1_match.fraction())
        );
        let enough_mapping = self.mapping.total >= MIN_MAPPING_UPDATES;
        let times_ok =
            |m: &Agreement| m.total > 0 && m.fraction().is_some_and(|f| f >= TIME_PASS_SHARE);
        let fail = (enough_mapping && mapping.is_some_and(|m| m < MAPPING_FAIL_SHARE))
            || (self.lap_match.total >= 2
                && self
                    .lap_match
                    .fraction()
                    .is_some_and(|f| f < PAIRED_FAIL_SHARE));
        if fail {
            o.verdict(Verdict::Fail, summary)
        } else if enough_mapping
            && mapping.is_some_and(|m| m >= MAPPING_PASS_SHARE)
            && times_ok(&self.lap_match)
            && times_ok(&self.s1_match)
        {
            o.verdict(Verdict::Pass, summary)
        } else {
            o.verdict(Verdict::Inconclusive, summary)
        }
    }
}

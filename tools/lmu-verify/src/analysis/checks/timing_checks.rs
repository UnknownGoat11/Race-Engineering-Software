//! Checks computed from the rates & timing section: native update rates (A-LMU-001, A-LMU-002) and the
//! ET freshness keys (A-LMU-031, A-LMU-059).

use super::{CheckResult, Outcome, Verdict, fmt_opt};
use crate::analysis::timing::TimingReport;

/// Telemetry rate accepted as "about 100 Hz" (Hz).
const TELEMETRY_HZ_PASS: (f64, f64) = (90.0, 110.0);
/// Telemetry rate outside this contradicts A-LMU-001 (Hz).
const TELEMETRY_HZ_PLAUSIBLE: (f64, f64) = (50.0, 200.0);
/// Scoring rate accepted as "about 5 Hz" (Hz).
const SCORING_HZ_PASS: (f64, f64) = (4.0, 6.0);
/// Scoring rate outside this contradicts A-LMU-002 (Hz).
const SCORING_HZ_PLAUSIBLE: (f64, f64) = (2.0, 10.0);
/// Telemetry updates needed for a rate verdict.
const MIN_TELEMETRY_UPDATES: u64 = 1_000;
/// Scoring updates needed for a rate verdict.
const MIN_SCORING_UPDATES: u64 = 50;
/// |telemetry ET − scoring ET| p99 accepted as "same clock" (s): scoring is published every ~200 ms.
const SAME_CLOCK_P99_S: f64 = 0.5;
/// |telemetry ET − scoring ET| p99 above this contradicts A-LMU-059 (s).
const DIFFERENT_CLOCK_P99_S: f64 = 2.0;

fn within(v: f64, (lo, hi): (f64, f64)) -> bool {
    (lo..=hi).contains(&v)
}

/// Native update rates (A-LMU-001 ≈ 100 Hz telemetry, A-LMU-002 ≈ 5 Hz scoring).
pub(super) fn rates(timing: Option<&TimingReport>) -> CheckResult {
    let mut o = Outcome::new(
        "update_rates",
        "Native update rates (telemetry ≈ 100 Hz, scoring ≈ 5 Hz)",
        &["A-LMU-001", "A-LMU-002"],
        &["source.telemetry_rate_hz", "source.scoring_rate_hz"],
    );
    let Some(t) = timing else {
        return o.verdict(Verdict::Inconclusive, "no snapshot was replayed");
    };
    let tel = t.telemetry.median_rate_hz;
    let sco = t.scoring.median_rate_hz;
    o.metric_opt("telemetry_median_hz", tel)
        .metric_opt("scoring_median_hz", sco)
        .metric("telemetry_updates", t.telemetry.updates as f64)
        .metric("scoring_updates", t.scoring.updates as f64);
    if let Some(i) = &t.telemetry.interval_ms {
        o.line(format!(
            "telemetry interval median {:.2} ms (p5 {:.2}, p95 {:.2}) over {} intervals; {} gaps > 3× median",
            i.median_ms, i.p5_ms, i.p95_ms, i.count, t.telemetry.gaps
        ));
    }
    if let Some(i) = &t.scoring.interval_ms {
        o.line(format!(
            "scoring interval median {:.1} ms (p5 {:.1}, p95 {:.1}) over {} intervals; {} gaps > 3× median",
            i.median_ms, i.p5_ms, i.p95_ms, i.count, t.scoring.gaps
        ));
    }
    let enough =
        t.telemetry.updates >= MIN_TELEMETRY_UPDATES && t.scoring.updates >= MIN_SCORING_UPDATES;
    let summary = format!(
        "telemetry {} Hz, scoring {} Hz (median intervals, capture clock)",
        fmt_opt(tel, 1),
        fmt_opt(sco, 2)
    );
    match (tel, sco) {
        (Some(tel), Some(sco)) if enough => {
            if !within(tel, TELEMETRY_HZ_PLAUSIBLE) || !within(sco, SCORING_HZ_PLAUSIBLE) {
                o.verdict(Verdict::Fail, summary)
            } else if within(tel, TELEMETRY_HZ_PASS) && within(sco, SCORING_HZ_PASS) {
                o.verdict(Verdict::Pass, summary)
            } else {
                o.verdict(Verdict::Inconclusive, summary)
            }
        }
        _ => o.verdict(
            Verdict::Inconclusive,
            format!("{summary}; too few updates for a verdict"),
        ),
    }
}

/// ET freshness keys: monotonic within a session (A-LMU-031) and telemetry / scoring on the same clock
/// (A-LMU-059).
pub(super) fn elapsed_time(timing: Option<&TimingReport>) -> CheckResult {
    let mut o = Outcome::new(
        "elapsed_time",
        "ET monotonic within a session; telemetry and scoring ET on one clock",
        &["A-LMU-031", "A-LMU-059"],
        &["time.source_et_s", "session.elapsed_s"],
    );
    let Some(t) = timing else {
        return o.verdict(Verdict::Inconclusive, "no snapshot was replayed");
    };
    let p99 = t.abs_telemetry_minus_scoring_et_s.map(|s| s.p99);
    let dup = t.dispositions.get("DUPLICATE_ET").copied().unwrap_or(0);
    o.metric("et_regressions", t.et_regressions as f64)
        .metric(
            "unexplained_et_regressions",
            t.unexplained_et_regressions as f64,
        )
        .metric("scoring_et_regressions", t.scoring_et_regressions as f64)
        .metric(
            "scoring_et_regressions_not_restart",
            t.scoring_et_regressions_not_restart as f64,
        )
        .metric("duplicate_et_snapshots", dup as f64)
        .metric_opt("abs_tel_minus_scoring_et_p99_s", p99)
        .metric_opt("drift_ppm", t.drift.weighted_drift_ppm);
    o.line(format!(
        "player ET regressions {} ({} unexplained); scoring ET regressions {} ({} not to a restart near 0)",
        t.et_regressions,
        t.unexplained_et_regressions,
        t.scoring_et_regressions,
        t.scoring_et_regressions_not_restart
    ));
    for r in t.regressions.iter().take(5) {
        o.line(format!(
            "regression at capture {:.1} s: ET {:.3} → {:.3} s ({})",
            r.at_capture_s,
            r.from_et_s,
            r.to_et_s,
            r.explanation.as_deref().unwrap_or("UNEXPLAINED")
        ));
    }
    if let Some(s) = &t.telemetry_minus_scoring_et_s {
        o.line(format!(
            "telemetry ET − scoring ET at scoring updates: median {:.3} s, p1 {:.3}, p99 {:.3}, max {:.3} (n = {})",
            s.median, s.p1, s.p99, s.max, s.count
        ));
    }
    o.line(format!(
        "snapshots with an unchanged player ET (no new sample): {dup}; ET vs capture clock drift {} ppm over {} segment(s) totalling {:.0} s",
        fmt_opt(t.drift.weighted_drift_ppm, 0),
        t.drift.segments,
        t.drift.total_s
    ));
    let summary = format!(
        "{} unexplained ET regressions, |tel − scoring ET| p99 {} s",
        t.unexplained_et_regressions,
        fmt_opt(p99, 3)
    );
    if t.unexplained_et_regressions > 0
        || t.scoring_et_regressions_not_restart > 0
        || p99.is_some_and(|p| p > DIFFERENT_CLOCK_P99_S)
    {
        o.verdict(Verdict::Fail, summary)
    } else if p99.is_some_and(|p| p <= SAME_CLOCK_P99_S)
        && t.telemetry.updates >= MIN_TELEMETRY_UPDATES
        && t.scoring.updates >= MIN_SCORING_UPDATES
    {
        o.verdict(Verdict::Pass, summary)
    } else {
        o.verdict(Verdict::Inconclusive, summary)
    }
}

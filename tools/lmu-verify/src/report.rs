//! The verification report: building it from capture analyses, rendering `report.md`, serialising
//! `report.json` and the short stdout summary. Output is deterministic (fixed section and check order,
//! registry order for channels, sorted maps, no wall-clock timestamps), so two runs over the same captures
//! produce byte-identical files.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::analysis::CaptureReport;
use crate::analysis::channels::ChannelRow;
use crate::analysis::checks::{CheckResult, Verdict};
use crate::error::VerifyError;

/// Report file names inside the output directory.
pub const REPORT_MD: &str = "report.md";
/// JSON report file name.
pub const REPORT_JSON: &str = "report.json";
/// A capture's evidence counts for a game version only when that version covers at least this share of
/// its snapshots (a capture briefly reporting another value at startup does not add that version).
const DOMINANT_VERSION_SHARE: f64 = 0.9;

/// Build information of the tool that produced the report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolInfo {
    /// Tool name.
    pub name: &'static str,
    /// Crate version.
    pub version: &'static str,
    /// Canonical telemetry schema version of the normalizer output.
    pub telemetry_schema_version: u16,
    /// Layout fingerprint the reader decodes.
    pub layout_fingerprint: String,
    /// Game versions verified by this build.
    pub verified_game_versions: Vec<i32>,
}

/// A check combined over every capture.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CombinedCheck {
    /// Check id.
    pub id: &'static str,
    /// Title.
    pub title: &'static str,
    /// Any FAIL fails; otherwise any PASS passes.
    pub verdict: Verdict,
    /// Verdict per capture, in capture order.
    pub per_capture: Vec<Verdict>,
    /// Assumptions the check settles.
    pub settles: Vec<&'static str>,
}

/// What the evidence supports for the docs (the tool never edits docs).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decision {
    /// Check id.
    pub check: &'static str,
    /// Combined verdict.
    pub verdict: Verdict,
    /// Assumption ids.
    pub assumptions: Vec<&'static str>,
    /// Channel-map rows.
    pub docs_rows: Vec<&'static str>,
    /// Game versions of the captures that decided the verdict (their own verdict equals the combined one),
    /// each capture counted only for a version covering at least 90 % of its snapshots.
    pub game_versions: Vec<i32>,
    /// Suggested action.
    pub action: String,
}

/// A registry range that real values fell outside of (summed over every capture).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RangeFinding {
    /// Channel id (`0x0001`).
    pub id: String,
    /// Canonical name.
    pub name: &'static str,
    /// Registry plausible range.
    pub registry_range: Option<(f64, f64)>,
    /// Samples rejected as out of range.
    pub rejected: u64,
    /// Published samples of the channel.
    pub samples: u64,
    /// Smallest and largest rejected value.
    pub extremes: Option<(f64, f64)>,
}

/// A continuous physics channel that never changed while the car moved (in at least one capture).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConstantFinding {
    /// Channel id (`0x0001`).
    pub id: String,
    /// Canonical name.
    pub name: &'static str,
    /// The constant value, when the channel never changed at all.
    pub value: Option<f64>,
    /// Longest constant run while moving (s), over every capture.
    pub longest_constant_moving_s: f64,
}

/// The whole report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VerificationReport {
    /// Tool build.
    pub tool: ToolInfo,
    /// LMU game versions seen in the captures.
    pub game_versions: Vec<i32>,
    /// Checks combined over all captures.
    pub combined: Vec<CombinedCheck>,
    /// Decisions this evidence supports.
    pub decisions: Vec<Decision>,
    /// Registry ranges that real values fell outside of (channel id order).
    pub range_findings: Vec<RangeFinding>,
    /// Continuous channels constant while moving (channel id order).
    pub constant_findings: Vec<ConstantFinding>,
    /// One report per capture, in command-line order.
    pub captures: Vec<CaptureReport>,
}

/// Builds the report from per-capture analyses.
pub fn build(captures: Vec<CaptureReport>) -> VerificationReport {
    let game_versions: Vec<i32> = captures
        .iter()
        .flat_map(|c| c.session.game_versions.keys().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut combined = Vec::new();
    if let Some(first) = captures.first() {
        for (index, check) in first.checks.iter().enumerate() {
            let per_capture: Vec<Verdict> = captures
                .iter()
                .map(|c| {
                    c.checks
                        .get(index)
                        .map_or(Verdict::Inconclusive, |r| r.verdict)
                })
                .collect();
            combined.push(CombinedCheck {
                id: check.id,
                title: check.title,
                verdict: Verdict::combine(per_capture.iter().copied()),
                per_capture,
                settles: check.settles.clone(),
            });
        }
    }
    let decisions = combined.iter().map(|c| decision(c, &captures)).collect();
    let (range_findings, constant_findings) = channel_findings(&captures);
    VerificationReport {
        tool: ToolInfo {
            name: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
            telemetry_schema_version: telemetry_model::TELEMETRY_SCHEMA_VERSION,
            layout_fingerprint: format!("{:#018x}", lmu_reader::offsets::LAYOUT_FINGERPRINT),
            verified_game_versions: lmu_reader::compat::VERIFIED_GAME_VERSIONS.to_vec(),
        },
        game_versions,
        combined,
        decisions,
        range_findings,
        constant_findings,
        captures,
    }
}

/// The decision for combined check `c`. Only the captures whose own verdict equals the combined one
/// decided it, so only their game versions are named: a PASS from a 1.4.2 capture says nothing about a
/// newer build whose capture was INCONCLUSIVE.
fn decision(c: &CombinedCheck, captures: &[CaptureReport]) -> Decision {
    let deciding: Vec<(&CaptureReport, &CheckResult)> = captures
        .iter()
        .filter_map(|cap| {
            cap.checks
                .iter()
                .find(|r| r.id == c.id && r.verdict == c.verdict)
                .map(|r| (cap, r))
        })
        .collect();
    let evidence = deciding.first().map_or("", |(_, r)| r.summary.as_str());
    let game_versions: Vec<i32> = deciding
        .iter()
        .filter_map(|(cap, _)| dominant_version(cap))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let unattributed = deciding
        .iter()
        .filter(|(cap, _)| dominant_version(cap).is_none())
        .count();
    let versions = format!(
        "gameVersion {}",
        game_versions
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    let unattributed_note = if unattributed > 0 {
        format!(
            "; {unattributed} deciding capture(s) without one known gameVersion (≥ {:.0} % of snapshots) not counted",
            100.0 * DOMINANT_VERSION_SHARE
        )
    } else {
        String::new()
    };
    let docs_rows = captures
        .first()
        .and_then(|cap| cap.checks.iter().find(|r| r.id == c.id))
        .map(|r| r.docs_rows.clone())
        .unwrap_or_default();
    let action = match c.verdict {
        Verdict::Pass if game_versions.is_empty() => format!(
            "passed, but no passing capture has one known gameVersion: cannot move to VERIFIED for a version{unattributed_note} ({evidence})"
        ),
        Verdict::Pass => format!(
            "can move to VERIFIED for {versions} ({} of {} capture(s) passed{unattributed_note}): {evidence}",
            deciding.len(),
            captures.len()
        ),
        Verdict::Fail => format!(
            "CONTRADICTED by this evidence{}: investigate before relying on it ({evidence})",
            if game_versions.is_empty() {
                String::new()
            } else {
                format!(" ({versions})")
            }
        ),
        Verdict::Inconclusive => format!("unchanged: needs more evidence ({evidence})"),
    };
    Decision {
        check: c.id,
        verdict: c.verdict,
        assumptions: c.settles.clone(),
        docs_rows,
        game_versions,
        action,
    }
}

/// The game version reported in at least [`DOMINANT_VERSION_SHARE`] of the capture's snapshots (`None`:
/// mixed, unknown, or not positive, e.g. a `gameVersion` of 0 before LMU fills it in).
fn dominant_version(cap: &CaptureReport) -> Option<i32> {
    let versions = &cap.session.game_versions;
    let total: u64 = versions.values().sum();
    versions
        .iter()
        .find(|&(&v, &n)| v > 0 && total > 0 && n as f64 >= DOMINANT_VERSION_SHARE * total as f64)
        .map(|(&v, _)| v)
}

/// Channel rows whose evidence contradicts the registry: values outside the plausible range, and
/// continuous channels frozen while moving. Merged over every capture, keyed (and ordered) by channel id.
fn channel_findings(captures: &[CaptureReport]) -> (Vec<RangeFinding>, Vec<ConstantFinding>) {
    let mut ranges: BTreeMap<String, RangeFinding> = BTreeMap::new();
    let mut constants: BTreeMap<String, ConstantFinding> = BTreeMap::new();
    for row in captures.iter().flat_map(|c| &c.channels) {
        if row.out_of_range > 0 {
            let f = ranges
                .entry(row.id.clone())
                .or_insert_with(|| RangeFinding {
                    id: row.id.clone(),
                    name: row.name,
                    registry_range: row.registry_range,
                    rejected: 0,
                    samples: 0,
                    extremes: None,
                });
            f.rejected += row.out_of_range;
            f.samples += row.counts.total();
            f.extremes = match (f.extremes, row.out_of_range_extremes) {
                (Some((a, b)), Some((c, d))) => Some((a.min(c), b.max(d))),
                (a, b) => a.or(b),
            };
        }
        if row.frozen_while_moving {
            let value = row.stats.as_ref().filter(|s| s.min == s.max).map(|s| s.min);
            let run = row.longest_constant_moving_s.unwrap_or(0.0);
            let f = constants
                .entry(row.id.clone())
                .or_insert_with(|| ConstantFinding {
                    id: row.id.clone(),
                    name: row.name,
                    value,
                    longest_constant_moving_s: run,
                });
            if f.value != value {
                f.value = None;
            }
            f.longest_constant_moving_s = f.longest_constant_moving_s.max(run);
        }
    }
    (
        ranges.into_values().collect(),
        constants.into_values().collect(),
    )
}

/// Writes `report.md` and `report.json` into `dir` (created if missing); returns their paths.
///
/// # Errors
/// [`VerifyError::Write`] if the directory or a file cannot be written, [`VerifyError::Json`] if the
/// report cannot be serialised.
pub fn write(report: &VerificationReport, dir: &Path) -> Result<(PathBuf, PathBuf), VerifyError> {
    fs::create_dir_all(dir).map_err(|source| VerifyError::Write {
        path: dir.to_path_buf(),
        source,
    })?;
    let md = dir.join(REPORT_MD);
    let json = dir.join(REPORT_JSON);
    fs::write(&md, render_markdown(report)).map_err(|source| VerifyError::Write {
        path: md.clone(),
        source,
    })?;
    fs::write(&json, to_json(report)?).map_err(|source| VerifyError::Write {
        path: json.clone(),
        source,
    })?;
    Ok((md, json))
}

/// The JSON report (pretty-printed, with a trailing newline).
///
/// # Errors
/// [`VerifyError::Json`] if serialisation fails.
pub fn to_json(report: &VerificationReport) -> Result<String, VerifyError> {
    let mut s = serde_json::to_string_pretty(report)?;
    s.push('\n');
    Ok(s)
}

/// A number with a precision that suits its magnitude (deterministic).
pub fn num(v: f64) -> String {
    if !v.is_finite() {
        return "n/a".to_owned();
    }
    let a = v.abs();
    if a == 0.0 {
        "0".to_owned()
    } else if a >= 10_000.0 {
        format!("{v:.0}")
    } else if a >= 100.0 {
        format!("{v:.1}")
    } else if a >= 1.0 {
        format!("{v:.3}")
    } else {
        format!("{v:.4}")
    }
}

fn opt(v: Option<f64>) -> String {
    v.map_or_else(|| "–".to_owned(), num)
}

/// Escapes a table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// The short stdout summary: per capture its replay status, rates and the check table.
pub fn summary_text(report: &VerificationReport) -> String {
    let mut s = String::new();
    for c in &report.captures {
        let _ = writeln!(s, "capture {}: {}", c.file, replay_line(c));
        if let Some(t) = &c.timing {
            let _ = writeln!(
                s,
                "  telemetry {} Hz (median), scoring {} Hz (median), duration {:.1} s",
                opt(t.telemetry.median_rate_hz),
                opt(t.scoring.median_rate_hz),
                t.capture_duration_s
            );
        }
    }
    let _ = writeln!(s, "\n{:<26} {:<13} settles", "check", "verdict");
    for c in &report.combined {
        let _ = writeln!(
            s,
            "{:<26} {:<13} {}",
            c.id,
            c.verdict.as_str(),
            c.settles.join(", ")
        );
    }
    s
}

fn replay_line(c: &CaptureReport) -> String {
    match &c.replay.stopped {
        None => format!(
            "complete, {} frame records, {} snapshots",
            c.replay.frames_read, c.replay.snapshots
        ),
        Some(stop) => format!(
            "STOPPED EARLY ({}) at frame {} after {} snapshots: {}",
            stop.kind,
            stop.frame_index
                .map_or_else(|| "header".to_owned(), |f| f.to_string()),
            stop.snapshots_before,
            stop.message
        ),
    }
}

/// Renders `report.md`.
pub fn render_markdown(report: &VerificationReport) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# LMU Gate-1 verification report\n");
    let _ = writeln!(
        s,
        "Generated by `{} {}` (telemetry schema {}, layout fingerprint `{}`, verified game versions {:?}). \
         Captures are replayed through `lmu-reader` and `lmu-normalizer` exactly as the live app does. \
         Personal names are redacted as `<name>`. This tool never edits the docs: the decisions below are \
         suggestions for a reviewer.\n",
        report.tool.name,
        report.tool.version,
        report.tool.telemetry_schema_version,
        report.tool.layout_fingerprint,
        report.tool.verified_game_versions
    );
    let _ = writeln!(s, "## Gate-1 checks\n");
    let _ = writeln!(s, "| Check | Verdict | Settles | Finding |");
    let _ = writeln!(s, "|---|---|---|---|");
    for c in &report.combined {
        let finding = report
            .decisions
            .iter()
            .find(|d| d.check == c.id)
            .map(|d| d.action.as_str())
            .unwrap_or("");
        let _ = writeln!(
            s,
            "| {} (`{}`) | **{}** | {} | {} |",
            cell(c.title),
            c.id,
            c.verdict.as_str(),
            c.settles.join(", "),
            cell(finding)
        );
    }
    let _ = writeln!(s, "\n## Decisions this evidence supports\n");
    for verdict in [Verdict::Pass, Verdict::Fail, Verdict::Inconclusive] {
        let items: Vec<&Decision> = report
            .decisions
            .iter()
            .filter(|d| d.verdict == verdict)
            .collect();
        if items.is_empty() {
            continue;
        }
        let heading = match verdict {
            Verdict::Pass => "Passed (can move to VERIFIED for the game versions named)",
            Verdict::Fail => "Contradicted (fix or re-check before relying on them)",
            Verdict::Inconclusive => "Unchanged (more evidence needed)",
        };
        let _ = writeln!(s, "**{heading}**\n");
        for d in items {
            let assumptions = if d.assumptions.is_empty() {
                "channel-map rows".to_owned()
            } else {
                d.assumptions.join(", ")
            };
            let _ = writeln!(
                s,
                "- {} (`{}`; rows: {}): {}",
                assumptions,
                d.check,
                d.docs_rows
                    .iter()
                    .map(|r| format!("`{r}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                d.action
            );
        }
        let _ = writeln!(s);
    }
    render_channel_findings(&mut s, report);
    for (i, c) in report.captures.iter().enumerate() {
        render_capture(&mut s, i + 1, c);
    }
    s
}

fn render_channel_findings(s: &mut String, report: &VerificationReport) {
    if !report.range_findings.is_empty() {
        let _ = writeln!(
            s,
            "**Registry ranges hit by real values** (each rejection was published INVALID; review per row: widen the \
             range, fix the unit or enum, or confirm the rejection was right, e.g. an impact spike)\n"
        );
        for f in &report.range_findings {
            let range = f.registry_range.map_or_else(
                || "–".to_owned(),
                |(lo, hi)| format!("{}..{}", num(lo), num(hi)),
            );
            let seen = f.extremes.map_or_else(
                || "–".to_owned(),
                |(lo, hi)| format!("{}..{}", num(lo), num(hi)),
            );
            let _ = writeln!(
                s,
                "- `{}` ({}): {} of {} samples ({:.2} %) outside the registry range {range}; rejected values {seen}",
                f.name,
                f.id,
                f.rejected,
                f.samples,
                crate::analysis::channels::percent(f.rejected, f.samples),
            );
        }
        let _ = writeln!(s);
    }
    if !report.constant_findings.is_empty() {
        let _ = writeln!(
            s,
            "**Continuous channels constant while moving** (published VALID; candidates for UNAVAILABLE / \
             NOT_SUPPORTED on this car or game version, or for a mapping fault; a setup value or a fixed step \
             can legitimately be constant)\n"
        );
        for f in &report.constant_findings {
            let value = f.value.map_or_else(
                || "changed only while stationary".to_owned(),
                |v| format!("always {}", num(v)),
            );
            let _ = writeln!(
                s,
                "- `{}` ({}): {value}; longest constant run while moving {:.1} s",
                f.name, f.id, f.longest_constant_moving_s
            );
        }
        let _ = writeln!(s);
    }
}

fn render_capture(s: &mut String, n: usize, c: &CaptureReport) {
    let _ = writeln!(s, "## Capture {n}: `{}`\n", cell(&c.file));
    let _ = writeln!(
        s,
        "- Size: {} bytes\n- Replay: {}",
        c.file_bytes
            .map_or_else(|| "unknown".to_owned(), |b| b.to_string()),
        replay_line(c)
    );
    if let Some(stop) = &c.replay.stopped {
        let _ = writeln!(
            s,
            "- **The replay stopped before the end of the file** ({}): frame index {}, after {} snapshots ({} s of capture). Everything below covers only the part before that point.",
            stop.kind,
            stop.frame_index
                .map_or_else(|| "header".to_owned(), |f| f.to_string()),
            stop.snapshots_before,
            opt(stop.last_capture_s)
        );
    }
    render_session(s, c);
    render_timing(s, c);
    render_checks(s, c);
    render_channels(s, &c.channels);
    render_census(s, c);
}

fn render_session(s: &mut String, c: &CaptureReport) {
    let f = &c.session;
    let _ = writeln!(s, "\n### Session facts\n");
    let _ = writeln!(s, "| Item | Value |\n|---|---|");
    let versions: Vec<String> = f
        .game_versions
        .iter()
        .map(|(v, n)| format!("{v} ({n} snapshots)"))
        .collect();
    let rows = [
        ("Game version(s)", versions.join(", ")),
        (
            "Verified versions (this build)",
            format!("{:?}", f.verified_versions),
        ),
        (
            "Snapshots with a verified version",
            f.version_verified_snapshots.to_string(),
        ),
        (
            "Interface fingerprint",
            format!(
                "`{}` ({} bytes layout)",
                f.layout_fingerprint, f.layout_size
            ),
        ),
        (
            "Compatibility verdict",
            format!(
                "**{}** (overall {}, {} frames checked, {} rejected)",
                f.compatibility.verdict,
                f.compatibility.overall.as_deref().unwrap_or("–"),
                f.compatibility.frames_checked,
                f.compatibility.frames_rejected
            ),
        ),
        ("Sync modes", map_line(&f.sync_modes)),
        ("Lock modes", map_line(&f.lock_modes)),
        ("Player identification", map_line(&f.player_identity)),
        (
            "Ambient / track temperature",
            format!(
                "{} / {} °C (median at scoring updates)",
                opt(f.ambient_temp_c.map(|t| t.median)),
                opt(f.track_temp_c.map(|t| t.median))
            ),
        ),
        ("Snapshots in realtime", f.realtime_snapshots.to_string()),
        ("Event codes", map_line(&f.event_codes)),
        (
            "Undocumented event values (first 16)",
            if f.other_event_values.is_empty() {
                map_line::<u64>(&BTreeMap::new())
            } else {
                format!(
                    "{}{}",
                    f.other_event_values
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    if f.other_event_values_not_listed > 0 {
                        format!(
                            " (+ {} occurrences of further values)",
                            f.other_event_values_not_listed
                        )
                    } else {
                        String::new()
                    }
                )
            },
        ),
        ("Normalizer notices", map_line(&f.notices)),
        ("Session events", map_line(&f.session_events)),
        (
            "Names redacted",
            format!(
                "{}{}",
                f.names_redacted,
                if f.names_overflowed {
                    " (LIMIT REACHED: redaction may be incomplete, do not share)"
                } else {
                    ""
                }
            ),
        ),
    ];
    for (k, v) in rows {
        let _ = writeln!(s, "| {k} | {} |", cell(&v));
    }
    if !f.capture_meta.is_empty() {
        let _ = writeln!(s, "\nCapture header: {}", cell(&f.capture_meta.join("; ")));
    }
    let _ = writeln!(
        s,
        "\n| Start (capture s) | Session | Track | Vehicle | Class (telemetry) | Class (scoring) | Snapshots |\n|---|---|---|---|---|---|---|"
    );
    for seg in &f.segments {
        let _ = writeln!(
            s,
            "| {:.1} | {} | {} | {} | {} | {} | {} |",
            seg.start_capture_s,
            cell(&seg.session_type),
            cell(&seg.track),
            cell(&seg.vehicle),
            cell(&seg.vehicle_class),
            cell(&seg.class_name),
            seg.snapshots
        );
    }
    if f.segments_not_listed > 0 {
        let _ = writeln!(
            s,
            "\n{} further segment changes not listed.",
            f.segments_not_listed
        );
    }
    let _ = writeln!(
        s,
        "\nNormalizer counters: {} snapshots, {} frames published, {} duplicate ET, {} ET regressions ({} session-end rewinds, {} teleport ET step-backs), {} non-finite ET, {} teleports, {} realtime entries, {} pit-service samples, {} no-player snapshots, {} invalid samples, {} mapping errors, {} scoring updates ({} rejected), {} session resets.",
        c.normalizer.snapshots,
        c.normalizer.frames_published,
        c.normalizer.duplicate_frames,
        c.normalizer.et_regressions,
        c.normalizer.session_end_rewinds,
        c.normalizer.teleport_et_step_backs,
        c.normalizer.non_finite_et,
        c.normalizer.teleports,
        c.normalizer.realtime_entries,
        c.normalizer.pit_service_samples,
        c.normalizer.no_player_snapshots,
        c.normalizer.invalid_samples,
        c.normalizer.mapping_errors,
        c.normalizer.scoring_updates,
        c.normalizer.scoring_rejected,
        c.normalizer.session_resets
    );
}

fn map_line<V: std::fmt::Display>(m: &std::collections::BTreeMap<String, V>) -> String {
    if m.is_empty() {
        return "–".to_owned();
    }
    m.iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_timing(s: &mut String, c: &CaptureReport) {
    let _ = writeln!(s, "\n### Rates & timing\n");
    let Some(t) = &c.timing else {
        let _ = writeln!(s, "No snapshot was replayed.");
        return;
    };
    let _ = writeln!(
        s,
        "Capture duration {:.1} s, {} snapshots, {} connection(s). Intervals on the capture clock (the capturing reader's monotonic clock right after each copy); never across a reconnection.\n",
        t.capture_duration_s, t.snapshots, t.connections
    );
    let _ = writeln!(
        s,
        "| Stream | Updates | Median rate (Hz) | Min | p5 | Median | p95 | Max (ms) | Gap threshold (ms) | Gaps |\n|---|---|---|---|---|---|---|---|---|---|"
    );
    for (name, r) in [
        ("Telemetry (new player sample)", &t.telemetry),
        ("Scoring (block changed)", &t.scoring),
        ("All snapshots", &t.all_snapshots),
    ] {
        let i = r.interval_ms;
        let _ = writeln!(
            s,
            "| {name} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            r.updates,
            opt(r.median_rate_hz),
            opt(i.map(|i| i.min_ms)),
            opt(i.map(|i| i.p5_ms)),
            opt(i.map(|i| i.median_ms)),
            opt(i.map(|i| i.p95_ms)),
            opt(i.map(|i| i.max_ms)),
            opt(r.gap_threshold_ms),
            r.gaps
        );
    }
    for (name, r) in [("Telemetry", &t.telemetry), ("Scoring", &t.scoring)] {
        if r.largest_gaps.is_empty() {
            continue;
        }
        let gaps: Vec<String> = r
            .largest_gaps
            .iter()
            .take(10)
            .map(|g| {
                format!(
                    "{} ms at {:.1} s (ET {} → {})",
                    num(g.length_ms),
                    g.at_capture_s,
                    opt(g.et_before_s),
                    opt(g.et_after_s)
                )
            })
            .collect();
        let _ = writeln!(
            s,
            "\n{name} gaps > 3× median (largest first): {}",
            gaps.join("; ")
        );
    }
    let _ = writeln!(s);
    let et = |i: Option<crate::analysis::timing::IntervalStats>| {
        i.map_or_else(
            || "–".to_owned(),
            |i| {
                format!(
                    "median {} ms (p5 {}, p95 {}, max {})",
                    num(i.median_ms),
                    num(i.p5_ms),
                    num(i.p95_ms),
                    num(i.max_ms)
                )
            },
        )
    };
    let lines = [
        format!(
            "Player ET step between telemetry updates: {}",
            et(t.et_step_ms)
        ),
        format!("mDeltaTime: {}", et(t.delta_time_ms)),
        format!(
            "ET monotonicity: {} player ET regressions ({} unexplained), {} scoring ET regressions ({} not to a restart, {} session-end rewinds), {} non-finite scoring ET",
            t.et_regressions,
            t.unexplained_et_regressions,
            t.scoring_et_regressions,
            t.scoring_et_regressions_not_restart,
            t.scoring_et_session_end_rewinds,
            t.scoring_et_non_finite
        ),
        format!(
            "Duplicate frames: {} byte-identical frame records dropped by the processor, {} generic-only snapshots, normalizer dispositions {}",
            t.reader.byte_identical_duplicates,
            t.reader.generic_only_frames,
            map_line(&t.dispositions)
        ),
        format!(
            "Reader counters: {} telemetry gaps (> 1.5 × mDeltaTime, {} samples skipped), {} player found by mID, {} player unverified, {} rejected frames",
            t.reader.telemetry_gaps,
            t.reader.telemetry_samples_skipped,
            t.reader.player_found_by_id_frames,
            t.reader.player_unverified_frames,
            t.reader.rejected_frames
        ),
        format!(
            "Telemetry ET − scoring ET at scoring updates: {}",
            t.telemetry_minus_scoring_et_s.map_or_else(
                || "–".to_owned(),
                |x| format!(
                    "median {} s, p1 {}, p99 {}, min {}, max {}",
                    num(x.median),
                    num(x.p1),
                    num(x.p99),
                    num(x.min),
                    num(x.max)
                )
            )
        ),
        format!(
            "ET vs capture-clock drift: {} ppm (duration-weighted over {} segments, {:.0} s; min {}, max {}); per-update |ΔET − Δcapture| {}",
            opt(t.drift.weighted_drift_ppm),
            t.drift.segments,
            t.drift.total_s,
            opt(t.drift.min_drift_ppm),
            opt(t.drift.max_drift_ppm),
            et(t.drift.step_deviation_ms)
        ),
    ];
    for l in lines {
        let _ = writeln!(s, "- {}", cell(&l));
    }
    for r in t.regressions.iter().take(10) {
        let _ = writeln!(
            s,
            // Three decimals: a 10 ms step-back must stay visible (315.830 → 315.820).
            "- ET regression at capture {:.1} s: {:.3} → {:.3} s ({})",
            r.at_capture_s,
            r.from_et_s,
            r.to_et_s,
            r.describe()
        );
    }
}

fn render_checks(s: &mut String, c: &CaptureReport) {
    let _ = writeln!(s, "\n### Gate-1 checks (this capture)\n");
    let _ = writeln!(
        s,
        "| Check | Verdict | Settles | Evidence |\n|---|---|---|---|"
    );
    for r in &c.checks {
        let _ = writeln!(
            s,
            "| {} | **{}** | {} | {} |",
            cell(r.title),
            r.verdict.as_str(),
            r.settles.join(", "),
            cell(&r.summary)
        );
    }
    for r in &c.checks {
        render_check_detail(s, r);
    }
}

fn render_check_detail(s: &mut String, r: &CheckResult) {
    let _ = writeln!(
        s,
        "\n#### {} — {} (`{}`)\n",
        r.verdict.as_str(),
        r.title,
        r.id
    );
    let _ = writeln!(s, "{}\n", r.summary);
    for e in &r.evidence {
        let _ = writeln!(s, "- {e}");
    }
}

fn render_channels(s: &mut String, rows: &[ChannelRow]) {
    let _ = writeln!(s, "\n### Per-channel table\n");
    let _ = writeln!(
        s,
        "One sample per published frame. Rate = distinct-value changes per second of LMU time while valid. \
         Statistics over valid values (percentiles exact for ≤ 1,024 distinct values, else P² estimates). \
         'Frozen' = a continuous physics channel that never changed over ≥ 10 s of moving time.\n"
    );
    let _ = writeln!(
        s,
        "| Id | Channel | Unit | Source / provenance | Valid % | Invalid % | Unavail. % | Not supp. % | Stale % | Rate (Hz) | Min | p1 | Median | p99 | Max | Rejections | Out of range | Longest constant moving (s) | Frozen |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
    );
    let mut never_valid = Vec::new();
    for r in rows {
        if r.counts.valid == 0 && r.counts.invalid == 0 {
            never_valid.push(format!(
                "`{}` ({})",
                r.name,
                if r.counts.not_supported == r.counts.total() {
                    "not supported"
                } else {
                    "unavailable"
                }
            ));
            continue;
        }
        let st = r.stats;
        let rejections = if r.rejections.is_empty() {
            "–".to_owned()
        } else {
            map_line(&r.rejections)
        };
        let oor = match r.out_of_range_extremes {
            Some((lo, hi)) => format!(
                "{} ({}..{}; range {})",
                r.out_of_range,
                num(lo),
                num(hi),
                r.registry_range.map_or_else(
                    || "–".to_owned(),
                    |(a, b)| format!("{}..{}", num(a), num(b))
                )
            ),
            None => r.out_of_range.to_string(),
        };
        let _ = writeln!(
            s,
            "| {} | `{}` | {} | {} / {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            r.id,
            r.name,
            cell(r.unit),
            r.source,
            r.provenance,
            r.valid_pct,
            r.invalid_pct,
            r.unavailable_pct,
            r.not_supported_pct,
            r.stale_pct,
            opt(r.change_rate_hz),
            opt(st.map(|x| x.min)),
            opt(st.map(|x| x.p1)),
            opt(st.map(|x| x.median)),
            opt(st.map(|x| x.p99)),
            opt(st.map(|x| x.max)),
            cell(&rejections),
            cell(&oor),
            opt(r.longest_constant_moving_s),
            if r.frozen_while_moving { "**yes**" } else { "" }
        );
    }
    if !never_valid.is_empty() {
        let _ = writeln!(
            s,
            "\nChannels never valid in this capture ({}): {}",
            never_valid.len(),
            never_valid.join(", ")
        );
    }
}

fn render_census(s: &mut String, c: &CaptureReport) {
    let census = &c.census;
    let _ = writeln!(s, "\n### Raw NaN / ±Inf census\n");
    let _ = writeln!(
        s,
        "Raw structs inspected: {}; float values checked: {}; non-finite values: {}.",
        map_line(&owned(&census.structs_checked)),
        map_line(&owned(&census.floats_checked)),
        census.total_non_finite()
    );
    if !census.non_finite.is_empty() {
        let _ = writeln!(s, "\n| Raw field | NaN | +Inf | −Inf |\n|---|---|---|---|");
        for (path, n) in &census.non_finite {
            let _ = writeln!(
                s,
                "| `{path}` | {} | {} | {} |",
                n.nan, n.pos_inf, n.neg_inf
            );
        }
    }
    if !census.reserved_non_zero.is_empty() {
        let _ = writeln!(
            s,
            "\nReserved / expansion bytes in use (non-zero): {}",
            map_line(&census.reserved_non_zero)
        );
    }
    let _ = writeln!(s);
}

fn owned(
    m: &std::collections::BTreeMap<&'static str, u64>,
) -> std::collections::BTreeMap<String, u64> {
    m.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_formatted_by_magnitude() {
        assert_eq!(num(0.0), "0");
        assert_eq!(num(0.012_345), "0.0123");
        assert_eq!(num(9.806_65), "9.807");
        assert_eq!(num(291.16), "291.2");
        assert_eq!(num(170_000.4), "170000");
        assert_eq!(num(f64::NAN), "n/a");
        assert_eq!(cell("a|b\nc"), "a\\|b c");
    }
}

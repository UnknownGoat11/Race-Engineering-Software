//! Malformed, truncated and missing captures never panic: the report says where the replay stopped.
//! Output is deterministic and personal names never reach it.

mod common;

use std::path::Path;

use common::{Scenario, analyze, capture};
use lmu_verify::analysis::analyze_capture;
use lmu_verify::report;

/// 20 s from the garage: enough frames to cut, quick to replay.
fn short_capture() -> Vec<u8> {
    capture(&Scenario {
        max_frames: 2_000,
        ..Scenario::default()
    })
}

/// Bytes before the first frame record: magic, version, layout size, fingerprint, meta length, meta.
fn header_len(bytes: &[u8]) -> usize {
    const FIXED: usize = 8 + 2 + 4 + 8;
    let meta_len = u32::from_le_bytes(bytes[FIXED..FIXED + 4].try_into().expect("meta length"));
    FIXED + 4 + meta_len as usize
}

#[test]
fn truncated_capture_reports_where_it_stopped() {
    let bytes = short_capture();
    let cut = &bytes[..bytes.len() - 100];
    let report = analyze(cut);
    assert!(!report.replay.complete);
    let stop = report.replay.stopped.as_ref().expect("stop info");
    assert_eq!(stop.kind, "TRUNCATED");
    assert_eq!(stop.frame_index, Some(1_999), "{stop:?}");
    assert_eq!(stop.snapshots_before, report.replay.snapshots);
    assert!(
        report.replay.snapshots > 1_900,
        "{}",
        report.replay.snapshots
    );
    assert!(stop.last_capture_s.is_some_and(|s| s > 19.0), "{stop:?}");
    // Everything before the cut is analysed.
    assert_eq!(report.checks.len(), common::CHECK_IDS.len());
    let md = report::render_markdown(&report::build(vec![report]));
    assert!(
        md.contains("STOPPED EARLY (TRUNCATED) at frame 1999"),
        "{md}"
    );
}

#[test]
fn corrupt_record_length_stops_with_corrupt() {
    let mut bytes = short_capture();
    let first = header_len(&bytes);
    let record_len = u32::from_le_bytes(bytes[first..first + 4].try_into().expect("len")) as usize;
    // The third record announces 2 GiB.
    let third = first + 2 * (4 + record_len);
    bytes[third..third + 4].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    let report = analyze(&bytes);
    let stop = report.replay.stopped.as_ref().expect("stop info");
    assert_eq!(
        (stop.kind.as_str(), stop.frame_index),
        ("CORRUPT", Some(2)),
        "{stop:?}"
    );
    assert_eq!(report.replay.snapshots, 2);
}

#[test]
fn garbage_empty_and_missing_files_are_reported_not_panicked() {
    let garbage = analyze(b"this is not a capture at all, just some text bytes");
    assert_eq!(
        garbage
            .replay
            .stopped
            .as_ref()
            .map(|s| (s.kind.as_str(), s.frame_index)),
        Some(("BAD_MAGIC", None))
    );
    assert!(garbage.timing.is_none());

    let empty = analyze(b"");
    assert!(empty.replay.stopped.is_some());
    assert_eq!(empty.replay.snapshots, 0);

    let missing = analyze_capture(Path::new("definitely/not/here/missing.lmuraw"));
    assert_eq!(
        missing.replay.stopped.as_ref().map(|s| s.kind.as_str()),
        Some("IO")
    );
    assert_eq!(missing.file, "missing.lmuraw");

    // Every check still reports (INCONCLUSIVE without data) and the report renders.
    let built = report::build(vec![garbage, empty, missing]);
    assert!(
        built
            .combined
            .iter()
            .all(|c| c.verdict == lmu_verify::analysis::checks::Verdict::Inconclusive)
    );
    assert!(report::render_markdown(&built).contains("STOPPED EARLY (BAD_MAGIC) at frame header"));
    report::to_json(&built).expect("json");
}

#[test]
fn output_is_deterministic_and_names_are_redacted() {
    let bytes = short_capture();
    let a = report::build(vec![analyze(&bytes)]);
    let b = report::build(vec![analyze(&bytes)]);
    let (json_a, json_b) = (
        report::to_json(&a).expect("json"),
        report::to_json(&b).expect("json"),
    );
    assert_eq!(json_a, json_b);
    let (md_a, md_b) = (report::render_markdown(&a), report::render_markdown(&b));
    assert_eq!(md_a, md_b);
    for text in [&json_a, &md_a] {
        assert!(!text.contains("Synthetic Driver"), "a driver name leaked");
    }
    assert!(a.captures[0].session.names_redacted > 0);

    let dir = std::env::temp_dir().join(format!("lmu-verify-test-{}", std::process::id()));
    let (md_path, json_path) = report::write(&a, &dir).expect("write");
    assert_eq!(std::fs::read_to_string(&md_path).expect("md"), md_a);
    assert_eq!(std::fs::read_to_string(&json_path).expect("json"), json_a);
    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn garbage_lap_number_does_not_panic() {
    // mLapNumber = i32::MAX: `lap + 1` would overflow (and panic in debug builds).
    let report = analyze(&capture(&Scenario {
        max_frames: 2_000,
        faults: common::Faults {
            garbage_lap_number: true,
            ..common::Faults::default()
        },
        ..Scenario::default()
    }));
    assert!(report.replay.complete, "{:?}", report.replay.stopped);
    assert_eq!(report.checks.len(), common::CHECK_IDS.len());
}

#[test]
fn markdown_counters_show_how_discontinuities_were_handled() {
    // A reviewer of report.md must see whether an ET regression was a teleport step-back or a restart.
    let md = report::render_markdown(&report::build(vec![analyze(&short_capture())]));
    for counter in [
        "teleport ET step-backs",
        "non-finite ET",
        "teleports",
        "realtime entries",
        "pit-service samples",
        "session resets",
    ] {
        assert!(md.contains(counter), "missing {counter:?} in {md}");
    }
}

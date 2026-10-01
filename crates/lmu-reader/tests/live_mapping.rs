//! Live check against a running LMU, read-only (D-018). Ignored by default; run it with LMU on
//! track:
//!
//! ```text
//! cargo test -p lmu-reader --test live_mapping -- --ignored --nocapture
//! ```
//!
//! Without LMU it only confirms that the reader waits cleanly.

use std::time::{Duration, Instant};

use lmu_reader::{LmuError, LmuReader, ReaderConfig, ReaderState, SupportLevel};

/// How long frames are observed.
const OBSERVE_FOR: Duration = Duration::from_secs(5);
/// Bound of each poll.
const POLL: Duration = Duration::from_millis(200);

#[test]
#[ignore = "reads the live LMU_Data mapping (read-only); start LMU and get on track first"]
fn reads_the_live_mapping_when_lmu_is_running() {
    let mut reader = match LmuReader::open(ReaderConfig::default()) {
        Ok(reader) => reader,
        Err(LmuError::UnsupportedPlatform) => return,
        Err(e) => panic!("open failed: {e}"),
    };
    let deadline = Instant::now() + OBSERVE_FOR;
    let mut snapshots = 0u32;
    let mut telemetry_updates = 0u32;
    while Instant::now() < deadline {
        match reader.next_snapshot(POLL) {
            Ok(Some(snapshot)) => {
                snapshots += 1;
                telemetry_updates += u32::from(snapshot.telemetry_updated);
                assert_ne!(snapshot.support, SupportLevel::Unsupported);
            }
            Ok(None) => {}
            Err(e) => panic!("reader error: {e}"),
        }
    }
    let diagnostics = reader.diagnostics();
    if reader.state() == ReaderState::WaitingForLmu && snapshots == 0 {
        eprintln!(
            "LMU is not running; nothing to verify ({:?})",
            diagnostics.note
        );
        return;
    }
    let report = reader.compatibility();
    let stats = reader.stats();
    eprintln!(
        "state {} | snapshots {snapshots} | telemetry updates {telemetry_updates} | telemetry {:?} Hz | \
         scoring {:?} Hz | lock hold max {:?} ns | overall {:?} | gameVersion {:?}",
        reader.state(),
        stats.telemetry_rate.rate_hz(),
        stats.scoring_rate.rate_hz(),
        stats.lock_hold.max_ns(),
        report.overall,
        report.game_version,
    );
    for check in report.checks.iter().filter(|c| c.outcome.is_some()) {
        eprintln!("  {:?} {}: {}", check.outcome, check.title, check.detected);
    }
    assert!(
        report.is_decodable(),
        "{}",
        report.incompatibility_message()
    );
    assert!(snapshots > 0, "attached but no frame was decoded");
}

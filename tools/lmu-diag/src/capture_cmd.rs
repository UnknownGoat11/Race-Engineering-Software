//! `lmu-diag capture --seconds N --out file.lmuraw`: records the raw copied blocks and timestamps
//! (format documented in `lmu_reader::capture`) for offline normalizer tests.

use std::io::BufWriter;
use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lmu_reader::compat::{LAYOUT_SOURCE, PROBED_LMU_BUILD};
use lmu_reader::{CaptureWriter, FrameInfo, ReaderConfig, ReaderState};

use crate::live::create_output;
use crate::stop::STOP;
use crate::worker::{Msg, Worker};
use crate::{COMPONENT, DiagError};

/// How often progress is printed and the file flushed.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);
/// Status cadence requested from the reader thread.
const STATUS_EVERY: Duration = Duration::from_millis(500);

/// Records for `duration` into `out`.
pub(crate) fn run(
    duration: Duration,
    out: &Path,
    force: bool,
    allow_in_git: bool,
) -> Result<ExitCode, DiagError> {
    let file = create_output(out, force, allow_in_git)?;
    eprintln!(
        "lmu-diag: WARNING {} will contain driver names, the player name and server names from LMU (PII). \
         Keep it private; do not commit it or share it publicly.",
        out.display()
    );
    let config = ReaderConfig::default();
    let created_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let meta = format!(
        "tool: lmu-diag {}\ncreated_unix_ms: {created_unix_ms}\nlayout_source: {LAYOUT_SOURCE}\nprobed_build: {PROBED_LMU_BUILD}\n\
         clock: capture_mono_ns = nanoseconds since lmu-diag started (monotonic)\n\
         privacy: contains driver, player and server names (PII)\n",
        env!("CARGO_PKG_VERSION"),
    );
    let mut writer = CaptureWriter::new(BufWriter::new(file), &meta)?;
    let worker = Worker::spawn(config, true, STATUS_EVERY)?;
    let started = Instant::now();
    let mut next_progress = started + PROGRESS_EVERY;
    let mut state = ReaderState::WaitingForLmu;
    let mut first_error: Option<String> = None;

    let end = started.checked_add(duration).unwrap_or(started);
    while started.elapsed() < duration && !STOP.is_requested() {
        let wait = next_progress
            .min(end)
            .saturating_duration_since(Instant::now());
        match worker.recv_timeout(wait) {
            Ok(Msg::Frame(frame)) => {
                if let Some(raw) = &frame.raw {
                    writer.write_frame(&FrameInfo::of(&frame.snapshot), &raw.blocks())?;
                }
            }
            Ok(Msg::Status(status)) => state = status.diagnostics.state,
            Ok(Msg::Error(e)) => {
                tracing::warn!(component = COMPONENT, error = %e, "reader error during capture");
                eprintln!("lmu-diag: reader error: {e}");
                first_error.get_or_insert(e);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(DiagError::Worker("the reader thread stopped".to_owned()));
            }
        }
        if Instant::now() >= next_progress {
            next_progress += PROGRESS_EVERY;
            writer.flush()?;
            eprintln!(
                "lmu-diag: {:>4.0} s / {} s | state {state} | {} frames | {} dropped",
                started.elapsed().as_secs_f64(),
                duration.as_secs(),
                writer.frames(),
                worker.dropped_frames()
            );
        }
    }
    // Counted after the join: the reader thread may drop more until it stops.
    let dropped = worker.stop()?;
    let frames = writer.frames();
    writer.finish()?;
    println!("{}", summary_line(frames, dropped, out));
    if dropped > 0 {
        tracing::warn!(component = COMPONENT, dropped, "capture is incomplete");
        eprintln!(
            "lmu-diag: WARNING {dropped} frames were dropped (the writer fell behind); the capture has gaps (see the seq values)"
        );
    }
    if frames == 0 {
        eprintln!(
            "lmu-diag: no frames were captured (state {state}{}). Is LMU running and are you on track?",
            first_error.map_or_else(String::new, |e| format!(", error: {e}"))
        );
        return Ok(ExitCode::from(crate::EXIT_NOT_CHECKED));
    }
    Ok(ExitCode::SUCCESS)
}

/// The final report line: frames written and, when any were lost, how many were dropped.
pub(crate) fn summary_line(frames: u64, dropped: u64, out: &Path) -> String {
    if dropped == 0 {
        format!("lmu-diag: wrote {frames} frames to {}", out.display())
    } else {
        format!(
            "lmu-diag: wrote {frames} frames to {} ({dropped} frames DROPPED: incomplete)",
            out.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_reports_dropped_frames() {
        let out = Path::new("x.lmuraw");
        assert_eq!(
            summary_line(10, 0, out),
            "lmu-diag: wrote 10 frames to x.lmuraw"
        );
        assert!(summary_line(10, 3, out).contains("3 frames DROPPED"));
    }
}

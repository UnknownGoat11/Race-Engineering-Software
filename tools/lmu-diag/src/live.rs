//! `lmu-diag live`: refreshing terminal view plus optional JSONL log of every decoded frame.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use lmu_reader::{RawSnapshot, ReaderConfig};
use serde::Serialize;

use crate::stop::STOP;
use crate::view::{LiveInput, render_live};
use crate::worker::{Msg, Status, Worker};
use crate::{COMPONENT, DiagError};

/// ANSI "clear screen, cursor home".
const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
/// Reader errors kept on screen.
const MAX_ERRORS_SHOWN: usize = 3;

/// Arguments of `lmu-diag live`.
pub(crate) struct LiveArgs {
    pub(crate) log: Option<PathBuf>,
    pub(crate) refresh: Duration,
    pub(crate) duration: Option<Duration>,
    pub(crate) clear: bool,
    pub(crate) force: bool,
    pub(crate) allow_in_git: bool,
}

/// The git working tree that would contain a file created at `path`, if any (its directory or an
/// ancestor holds `.git`). `None` also when the directory does not exist (the open then fails).
pub(crate) fn git_work_tree_of(path: &Path) -> Option<PathBuf> {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let dir = dir.canonicalize().ok()?;
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Opens a new output file, refusing to overwrite unless `force`, and refusing a location inside a
/// git working tree unless `allow_in_git` (the files contain names: owner rule, keep recordings
/// under `D:\RaceEngineeringData` and never commit them).
pub(crate) fn create_output(
    path: &Path,
    force: bool,
    allow_in_git: bool,
) -> Result<File, DiagError> {
    if !allow_in_git && let Some(tree) = git_work_tree_of(path) {
        return Err(DiagError::InsideGitTree {
            path: path.to_path_buf(),
            tree,
        });
    }
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    options.open(path).map_err(|source| {
        if source.kind() == io::ErrorKind::AlreadyExists {
            DiagError::Exists(path.to_path_buf())
        } else {
            DiagError::File {
                path: path.to_path_buf(),
                source,
            }
        }
    })
}

/// One JSONL record: the decoded raw frame. Scoring is written when it changed (to keep the log
/// small) and on the first frame after any dropped frame, so a scoring change carried by a dropped
/// frame is never missing from the log.
/// NaN / infinite values appear as `null` (JSON has no NaN).
#[derive(Serialize)]
struct LogLine<'a> {
    seq: u64,
    capture_mono_ns: u64,
    connection_id: u64,
    telemetry_updated: bool,
    scoring_updated: bool,
    support: lmu_reader::SupportLevel,
    version_verified: bool,
    source: lmu_reader::FrameSource,
    generic: &'a lmu_reader::RawGeneric,
    telemetry_header: lmu_reader::TelemetryHeader,
    player: Option<&'a lmu_reader::PlayerTelemetry>,
    player_identity: lmu_reader::PlayerIdentity,
    scoring: Option<&'a lmu_reader::ScoringBlock>,
    /// Frames dropped before this line since the log started (cumulative; the UI fell behind).
    dropped_frames: u64,
}

impl<'a> LogLine<'a> {
    /// The record of `s`; `after_drop` forces the scoring block (a dropped frame may have carried
    /// a scoring change); `dropped` is the cumulative dropped-frame count.
    fn of(s: &'a RawSnapshot, after_drop: bool, dropped: u64) -> Self {
        Self {
            seq: s.seq,
            capture_mono_ns: s.capture_mono_ns,
            connection_id: s.connection_id,
            telemetry_updated: s.telemetry_updated,
            scoring_updated: s.scoring_updated,
            support: s.support,
            version_verified: s.version_verified,
            source: s.source,
            generic: &s.generic,
            telemetry_header: s.telemetry_header,
            player: s.player.as_ref(),
            player_identity: s.player_identity,
            scoring: (s.scoring_updated || after_drop).then_some(&*s.scoring),
            dropped_frames: dropped,
        }
    }
}

/// Whether frames were dropped between the previous logged frame and this one, whose
/// `dropped_before` is given; `seen` is the count stamped on the previous logged frame.
fn gap_before(seen: &mut u64, dropped_before: u64) -> bool {
    let gap = dropped_before > *seen;
    *seen = dropped_before;
    gap
}

struct JsonlLog {
    path: PathBuf,
    out: BufWriter<File>,
    frames: u64,
    /// Dropped-frame count when the previous line was written.
    dropped_seen: u64,
}

impl JsonlLog {
    fn create(path: &Path, force: bool, allow_in_git: bool) -> Result<Self, DiagError> {
        let file = create_output(path, force, allow_in_git)?;
        eprintln!(
            "lmu-diag: logging decoded frames to {} - the file contains driver, player and server names (PII); do not share it publicly.",
            path.display()
        );
        Ok(Self {
            path: path.to_path_buf(),
            out: BufWriter::new(file),
            frames: 0,
            dropped_seen: 0,
        })
    }

    /// Writes one frame; `dropped` is the count of frames dropped before it was sent
    /// ([`crate::worker::Frame::dropped_before`]).
    fn write(&mut self, s: &RawSnapshot, dropped: u64) -> Result<(), DiagError> {
        let after_drop = gap_before(&mut self.dropped_seen, dropped);
        let line = LogLine::of(s, after_drop, dropped);
        serde_json::to_writer(&mut self.out, &line)?;
        self.out
            .write_all(b"\n")
            .map_err(|source| self.io(source))?;
        self.frames += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), DiagError> {
        self.out.flush().map_err(|source| self.io(source))
    }

    fn io(&self, source: io::Error) -> DiagError {
        DiagError::File {
            path: self.path.clone(),
            source,
        }
    }
}

/// Runs the live view until Ctrl+C (handled: the reader is stopped and joined first) or the optional
/// duration ends.
pub(crate) fn run(args: &LiveArgs) -> Result<ExitCode, DiagError> {
    let mut log = args
        .log
        .as_deref()
        .map(|p| JsonlLog::create(p, args.force, args.allow_in_git))
        .transpose()?;
    let config = ReaderConfig::default();
    let clock = config.clock;
    let worker = Worker::spawn(config, false, args.refresh)?;
    let started = Instant::now();
    let mut latest: Option<RawSnapshot> = None;
    let mut status: Option<Status> = None;
    let mut errors: Vec<String> = Vec::new();
    let mut next_render = Instant::now();
    let mut stdout = io::stdout();

    while !STOP.is_requested() {
        let timeout = next_render.saturating_duration_since(Instant::now());
        match worker.recv_timeout(timeout) {
            Ok(Msg::Frame(frame)) => {
                if let Some(log) = log.as_mut() {
                    log.write(&frame.snapshot, frame.dropped_before)?;
                }
                latest = Some(frame.snapshot);
            }
            Ok(Msg::Status(s)) => status = Some(*s),
            Ok(Msg::Error(e)) => {
                tracing::warn!(component = COMPONENT, error = %e, "reader error");
                errors.push(e);
                if errors.len() > MAX_ERRORS_SHOWN {
                    errors.remove(0);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(DiagError::Worker("the reader thread stopped".to_owned()));
            }
        }
        let now = Instant::now();
        if now < next_render {
            continue;
        }
        next_render = now + args.refresh;
        if let Some(log) = log.as_mut() {
            // Bounded loss on Ctrl+C: at most one refresh interval of log lines.
            log.flush()?;
        }
        let screen = render_live(&LiveInput {
            status: status.as_ref(),
            snapshot: latest.as_ref(),
            now_ns: clock.now_ns(),
            errors: &errors,
            log: log.as_ref().map(|l| (l.path.as_path(), l.frames)),
        });
        let prefix = if args.clear { CLEAR_SCREEN } else { "" };
        write!(stdout, "{prefix}{screen}").map_err(DiagError::Stdout)?;
        if !args.clear {
            writeln!(stdout).map_err(DiagError::Stdout)?;
        }
        stdout.flush().map_err(DiagError::Stdout)?;
        if args.duration.is_some_and(|d| started.elapsed() >= d) {
            break;
        }
    }
    let dropped = worker.stop()?;
    if let Some(mut log) = log {
        log.flush()?;
        println!(
            "{}",
            crate::capture_cmd::summary_line(log.frames, dropped, &log.path)
        );
        if dropped > 0 {
            tracing::warn!(component = COMPONENT, dropped, "JSONL log is incomplete");
            eprintln!(
                "lmu-diag: WARNING {dropped} frames were dropped (the log writer fell behind); the log has gaps (see the seq values)"
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmu_reader::testutil::LayoutBuilder;

    fn json(line: &LogLine<'_>) -> serde_json::Value {
        serde_json::to_value(line).expect("json")
    }

    #[test]
    fn outputs_inside_a_git_working_tree_are_refused() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("refused-test-output.jsonl");
        assert!(
            git_work_tree_of(&path).is_some(),
            "the crate is in the repo"
        );
        let err = create_output(&path, false, false).expect_err("refused");
        assert!(matches!(err, DiagError::InsideGitTree { .. }), "{err}");
        assert!(err.to_string().contains("--allow-in-git"));
        assert!(!path.exists(), "nothing was created");
        assert!(
            git_work_tree_of(Path::new("Z:\\no\\such\\dir\\x.lmuraw")).is_none(),
            "a missing directory is left to the open to report"
        );
    }

    #[test]
    fn a_gap_is_reported_once_on_the_frame_after_it() {
        let mut seen = 0;
        assert!(!gap_before(&mut seen, 0));
        assert!(
            gap_before(&mut seen, 2),
            "two frames dropped before this one"
        );
        assert!(!gap_before(&mut seen, 2), "no new drop");
        assert!(gap_before(&mut seen, 3));
    }

    #[test]
    fn scoring_is_logged_on_the_first_frame_after_a_drop() {
        let mut s = crate::view::tests::snapshot_of(&LayoutBuilder::on_track_frame());
        s.scoring_updated = false;
        let quiet = json(&LogLine::of(&s, false, 0));
        assert!(quiet["scoring"].is_null(), "unchanged scoring is omitted");
        assert_eq!(quiet["dropped_frames"], 0);

        let after_drop = json(&LogLine::of(&s, true, 4));
        assert!(
            after_drop["scoring"].is_object(),
            "a dropped frame may have carried a scoring change"
        );
        assert_eq!(after_drop["dropped_frames"], 4);
    }
}

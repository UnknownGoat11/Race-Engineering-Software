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
}

/// Opens a new output file, refusing to overwrite unless `force`.
pub(crate) fn create_output(path: &Path, force: bool) -> Result<File, DiagError> {
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

/// One JSONL record: the decoded raw frame (scoring only when it changed, to keep the log small).
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
}

struct JsonlLog {
    path: PathBuf,
    out: BufWriter<File>,
    frames: u64,
}

impl JsonlLog {
    fn create(path: &Path, force: bool) -> Result<Self, DiagError> {
        let file = create_output(path, force)?;
        eprintln!(
            "lmu-diag: logging decoded frames to {} - the file contains driver, player and server names (PII); do not share it publicly.",
            path.display()
        );
        Ok(Self {
            path: path.to_path_buf(),
            out: BufWriter::new(file),
            frames: 0,
        })
    }

    fn write(&mut self, s: &RawSnapshot) -> Result<(), DiagError> {
        let line = LogLine {
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
            scoring: s.scoring_updated.then_some(&*s.scoring),
        };
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
        .map(|p| JsonlLog::create(p, args.force))
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
                    log.write(&frame.snapshot)?;
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
    worker.stop()?;
    if let Some(mut log) = log {
        log.flush()?;
        println!(
            "lmu-diag: wrote {} frames to {}",
            log.frames,
            log.path.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

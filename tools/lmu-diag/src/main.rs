//! `lmu-diag`: Gate-1 diagnostic viewer for LMU's native `LMU_Data` shared memory.
//!
//! Strictly read-only toward LMU (D-018): it uses `lmu_reader::LmuReader`, which maps `LMU_Data`
//! with `FILE_MAP_READ`, waits on the frame gate with `SYNCHRONIZE`, and takes the SDK reader lock
//! only for the duration of each copy. It never launches, configures or writes to LMU.
//!
//! Subcommands: `compat` (report once), `live` (refreshing view, optional JSONL log), `capture`
//! (raw `.lmuraw` recording for offline normalizer tests). See README.md for the Gate-1 procedure.

// This binary's purpose is terminal output (owner rule: println! only in lmu-diag's rendering).
#![allow(clippy::print_stdout)]

mod capture_cmd;
mod compat_cmd;
mod fmt;
mod live;
mod stop;
mod view;
mod worker;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use lmu_reader::{CaptureError, LmuError};
use thiserror::Error;

/// `tracing` component name.
pub(crate) const COMPONENT: &str = "lmu-diag";

/// Exit code: LMU not running / nothing could be checked.
pub(crate) const EXIT_NOT_CHECKED: u8 = 3;
/// Exit code: structural compatibility checks failed.
pub(crate) const EXIT_INCOMPATIBLE: u8 = 2;
/// Longest accepted `compat --wait-secs` (1 hour).
const MAX_WAIT_SECS: u64 = 3_600;
/// Longest accepted `live --seconds` / `capture --seconds` (24 hours).
const MAX_RUN_SECS: u64 = 86_400;
/// Accepted `live --refresh-ms` range.
const REFRESH_MS_RANGE: std::ops::RangeInclusive<u64> = 10..=60_000;

#[derive(Debug, Parser)]
#[command(
    name = "lmu-diag",
    version,
    about = "Read-only diagnostics for Le Mans Ultimate's LMU_Data shared memory"
)]
struct Cli {
    /// Log filter for stderr diagnostics (tracing EnvFilter syntax, e.g. `info` or `lmu_reader=debug`).
    #[arg(long, global = true, default_value = "warn")]
    log_level: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Attach once and print the compatibility report (exit 0 = decodable, 2 = incompatible,
    /// 3 = LMU not running / nothing checked).
    Compat {
        /// How long to wait for LMU and a first checked frame.
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(0..=MAX_WAIT_SECS))]
        wait_secs: u64,
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Refreshing live view of the decoded raw values with tentative conversions.
    Live {
        /// Also write every decoded frame to this JSONL file (contains driver names: PII).
        #[arg(long)]
        log: Option<PathBuf>,
        /// Screen refresh interval.
        #[arg(long, default_value_t = 250, value_parser = clap::value_parser!(u64).range(REFRESH_MS_RANGE))]
        refresh_ms: u64,
        /// Stop after this many seconds (default: run until Ctrl+C).
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=MAX_RUN_SECS))]
        seconds: Option<u64>,
        /// Do not clear the screen between refreshes (for terminals without ANSI support).
        #[arg(long)]
        no_clear: bool,
        /// Overwrite an existing log file.
        #[arg(long)]
        force: bool,
    },
    /// Record the raw copied blocks and timestamps to a `.lmuraw` file (contains driver names: PII).
    Capture {
        /// Recording length (Ctrl+C ends it early; the file stays valid).
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=MAX_RUN_SECS))]
        seconds: u64,
        /// Output file.
        #[arg(long)]
        out: PathBuf,
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
}

/// Errors of the diagnostic tool (the root cause is kept as `source`).
#[derive(Debug, Error)]
pub(crate) enum DiagError {
    /// The reader failed.
    #[error("LMU reader: {0}")]
    Reader(#[from] LmuError),
    /// A capture could not be written.
    #[error("capture: {0}")]
    Capture(#[from] CaptureError),
    /// A file could not be created or written.
    #[error("{path}: {source}")]
    File {
        /// The file.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: io::Error,
    },
    /// Terminal output failed.
    #[error("writing to the terminal failed: {0}")]
    Stdout(#[source] io::Error),
    /// JSON encoding failed.
    #[error("JSON encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    /// Refusing to overwrite an existing file.
    #[error("{0} already exists; pass --force to overwrite it")]
    Exists(PathBuf),
    /// The reader thread could not be started or stopped unexpectedly.
    #[error("reader thread: {0}")]
    Worker(String),
}

fn init_tracing(filter: &str) {
    let filter = tracing_subscriber::EnvFilter::try_new(filter)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    // Logs go to stderr so they never interleave with the rendered view on stdout.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .try_init();
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(&cli.log_level);
    // Ctrl+C / console close must not kill the reader thread inside LMU's lock (see `stop`).
    if let Err(e) = stop::install() {
        tracing::warn!(component = COMPONENT, error = %e, "cannot install the Ctrl+C handler");
        eprintln!(
            "lmu-diag: WARNING cannot install the Ctrl+C handler ({e}); stop with --seconds instead of Ctrl+C"
        );
    }
    let result = match cli.command {
        Command::Compat { wait_secs, json } => {
            compat_cmd::run(Duration::from_secs(wait_secs), json)
        }
        Command::Live {
            log,
            refresh_ms,
            seconds,
            no_clear,
            force,
        } => live::run(&live::LiveArgs {
            log,
            refresh: Duration::from_millis(refresh_ms),
            duration: seconds.map(Duration::from_secs),
            clear: !no_clear,
            force,
        }),
        Command::Capture {
            seconds,
            out,
            force,
        } => capture_cmd::run(Duration::from_secs(seconds), &out, force),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            tracing::error!(component = COMPONENT, error = %e, "lmu-diag failed");
            let mut source = std::error::Error::source(&e);
            eprintln!("lmu-diag: {e}");
            while let Some(cause) = source {
                eprintln!("  caused by: {cause}");
                source = cause.source();
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_from_the_command_line_are_bounded() {
        let parse = |args: &[&str]| Cli::try_parse_from(args.iter().copied());
        assert!(parse(&["lmu-diag", "compat", "--wait-secs", "18446744073709551615"]).is_err());
        assert!(
            parse(&[
                "lmu-diag",
                "capture",
                "--seconds",
                "18446744073709551615",
                "--out",
                "x"
            ])
            .is_err()
        );
        assert!(parse(&["lmu-diag", "capture", "--seconds", "0", "--out", "x"]).is_err());
        assert!(parse(&["lmu-diag", "live", "--refresh-ms", "0"]).is_err());
        assert!(parse(&["lmu-diag", "live", "--seconds", "999999999"]).is_err());
        assert!(parse(&["lmu-diag", "compat", "--wait-secs", "3600"]).is_ok());
        assert!(parse(&["lmu-diag", "capture", "--seconds", "60", "--out", "x"]).is_ok());
        assert!(parse(&["lmu-diag", "live"]).is_ok());
    }
}

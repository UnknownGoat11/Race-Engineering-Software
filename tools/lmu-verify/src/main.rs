//! `lmu-verify` command line: `report` (offline capture verification) and `rest-probe` (opt-in, GET-only,
//! loopback-only REST probe). See README.md.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use lmu_verify::rest::{self, BodyPolicy, LoopbackEndpoint, ProbeOptions};
use lmu_verify::{COMPONENT, VerifyError, analysis, report};
use tracing_subscriber::EnvFilter;

/// Smallest accepted spacing between REST requests (ms): the probe must never hammer LMU.
const MIN_SPACING_MS: u64 = 200;
/// Default spacing between REST requests (ms).
const DEFAULT_SPACING_MS: u64 = 500;

#[derive(Debug, Parser)]
#[command(
    name = "lmu-verify",
    version,
    about = "Offline Gate-1 evidence tool for Le Mans Ultimate captures (read-only toward LMU, D-018)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Replays .lmuraw captures through lmu-reader and lmu-normalizer and writes report.md + report.json.
    Report {
        /// Capture files written by `lmu-diag capture`.
        #[arg(required = true)]
        captures: Vec<PathBuf>,
        /// Output directory (created if missing).
        #[arg(long, default_value = "lmu-verify-report")]
        out: PathBuf,
        /// Print report.json to stdout instead of the short summary.
        #[arg(long)]
        json: bool,
    },
    /// GET-only probe of LMU's supplementary REST endpoints on http://127.0.0.1:6397 (loopback only).
    RestProbe {
        /// Output directory for the saved responses (created if missing).
        #[arg(long, default_value = "lmu-rest-probe")]
        out: PathBuf,
        /// Also GET /rest/garage/UIScreen/RepairAndRefuel (can make the in-game pit menu flicker).
        #[arg(long)]
        include_pit_menu: bool,
        /// Minimum spacing between requests in milliseconds (at least 200).
        #[arg(long, default_value_t = DEFAULT_SPACING_MS, value_parser = clap::value_parser!(u64).range(MIN_SPACING_MS..))]
        spacing_ms: u64,
        /// Save every body verbatim. By default JSON bodies are saved with name-like values redacted
        /// and non-JSON bodies are not saved: raw bodies hold driver, player and server names.
        #[arg(long)]
        keep_raw_bodies: bool,
    },
}

fn run(cli: Cli) -> Result<(), VerifyError> {
    let stdout = std::io::stdout();
    let write_err = |source| VerifyError::Write {
        path: PathBuf::from("<stdout>"),
        source,
    };
    match cli.command {
        Command::Report {
            captures,
            out,
            json,
        } => {
            let analyses = captures
                .iter()
                .map(|path| {
                    tracing::info!(component = COMPONENT, capture = %path.display(), "replaying capture");
                    analysis::analyze_capture(path)
                })
                .collect();
            let built = report::build(analyses);
            let (md, json_path) = report::write(&built, &out)?;
            let mut lock = stdout.lock();
            if json {
                lock.write_all(report::to_json(&built)?.as_bytes())
                    .map_err(write_err)?;
            } else {
                write!(lock, "{}", report::summary_text(&built)).map_err(write_err)?;
                writeln!(lock, "\nwrote {} and {}", md.display(), json_path.display())
                    .map_err(write_err)?;
            }
            Ok(())
        }
        Command::RestProbe {
            out,
            include_pit_menu,
            spacing_ms,
            keep_raw_bodies,
        } => {
            let options = ProbeOptions {
                include_pit_menu,
                spacing: Duration::from_millis(spacing_ms),
                timeout: rest::REQUEST_TIMEOUT,
            };
            let results = rest::probe(&LoopbackEndpoint::lmu(), &options);
            let policy = if keep_raw_bodies {
                BodyPolicy::Raw
            } else {
                BodyPolicy::Redacted
            };
            rest::save(&results, &out, policy)?;
            let mut lock = stdout.lock();
            write!(lock, "{}", rest::table(&results)).map_err(write_err)?;
            writeln!(lock, "\nsaved responses to {}", out.display()).map_err(write_err)?;
            if policy == BodyPolicy::Redacted {
                writeln!(
                    lock,
                    "(name-like values redacted in JSON bodies; non-JSON bodies not saved, see --keep-raw-bodies)"
                )
                .map_err(write_err)?;
            }
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(component = COMPONENT, error = %e, "lmu-verify failed");
            let _ = writeln!(std::io::stderr(), "error: {e}");
            ExitCode::FAILURE
        }
    }
}

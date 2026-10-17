//! `lmu-diag compat`: attach once, check, print the compatibility report.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use lmu_reader::{
    CompatibilityReport, LmuReader, ReaderConfig, ReaderDiagnostics, ReaderState, SupportLevel,
};
use serde::Serialize;

use crate::stop::STOP;
use crate::view::render_compat;
use crate::{COMPONENT, DiagError, EXIT_INCOMPATIBLE, EXIT_NOT_CHECKED};

/// Bound of each reader poll while waiting.
const POLL: Duration = Duration::from_millis(200);

#[derive(Serialize)]
struct CompatOutput<'a> {
    diagnostics: &'a ReaderDiagnostics,
    report: &'a CompatibilityReport,
}

/// Waits up to `wait` for LMU and one checked frame, then prints the report.
pub(crate) fn run(wait: Duration, json: bool) -> Result<ExitCode, DiagError> {
    // Counted while the reader exists, so a console close waits for it (see `stop`).
    let _activity = STOP.reader_activity();
    let mut reader = LmuReader::open(ReaderConfig::default())?;
    let deadline = Instant::now()
        .checked_add(wait)
        .unwrap_or_else(Instant::now);
    while Instant::now() < deadline && !STOP.is_requested() {
        match reader.next_snapshot(POLL) {
            Ok(Some(_)) => break,
            Ok(None) => {
                // Attached and checked, but no new frame (menus / pause): the report is complete.
                if reader.state() != ReaderState::WaitingForLmu
                    && reader.compatibility().frames_checked > 0
                {
                    break;
                }
            }
            Err(e) if e.is_incompatible() => break,
            Err(e) => {
                tracing::warn!(component = COMPONENT, error = %e, "attach attempt failed; retrying")
            }
        }
    }
    let report = reader.compatibility();
    let diagnostics = reader.diagnostics();
    reader.close();
    if json {
        let text = serde_json::to_string_pretty(&CompatOutput {
            diagnostics: &diagnostics,
            report: &report,
        })?;
        println!("{text}");
    } else {
        print!("{}", render_compat(&diagnostics, &report));
    }
    Ok(match report.overall {
        None => ExitCode::from(EXIT_NOT_CHECKED),
        Some(SupportLevel::Unsupported) => ExitCode::from(EXIT_INCOMPATIBLE),
        Some(_) => ExitCode::SUCCESS,
    })
}

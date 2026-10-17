//! `lmu-relay` binary: see [`lmu_relay::cli`] for the commands and their environment variables.

use std::process::ExitCode;

use clap::Parser as _;
use lmu_relay::cli::{Cli, init_logging, log_format, run};

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(log_format(&cli));
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            tracing::error!(component = "relay.cli", error = %e, "cannot start the async runtime");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(component = "relay.cli", error = %e, cause = %error_chain(&e), "lmu-relay failed");
            ExitCode::FAILURE
        }
    }
}

/// The `source()` chain of an error, joined with ": " (root cause last).
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = Vec::new();
    let mut current = error.source();
    while let Some(e) = current {
        parts.push(e.to_string());
        current = e.source();
    }
    parts.join(": ")
}

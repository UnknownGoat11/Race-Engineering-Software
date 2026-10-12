//! `cargo run -p gen-ts` regenerates `apps/desktop/src/generated/` from `telemetry-model`.
//! `cargo run -p gen-ts -- --check` only reports stale files (exit code 1 if any).

use std::io::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let check_only = std::env::args().skip(1).any(|arg| arg == "--check");
    let root = gen_ts::repo_root();
    let files = gen_ts::generate();
    let mut stdout = std::io::stdout().lock();
    if check_only {
        return match gen_ts::stale(&root, &files) {
            Ok(problems) if problems.is_empty() => {
                let _ = writeln!(
                    stdout,
                    "gen-ts: {} generated files are up to date",
                    files.len()
                );
                ExitCode::SUCCESS
            }
            Ok(problems) => {
                for problem in problems {
                    let _ = writeln!(stdout, "gen-ts: {problem}");
                }
                let _ = writeln!(stdout, "gen-ts: run cargo run -p gen-ts");
                ExitCode::FAILURE
            }
            Err(e) => {
                let _ = writeln!(stdout, "gen-ts: cannot read the generated files: {e}");
                ExitCode::FAILURE
            }
        };
    }
    match gen_ts::write(&root, &files) {
        Ok(changed) => {
            let _ = writeln!(
                stdout,
                "gen-ts: {} generated files, {changed} written or removed",
                files.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            let _ = writeln!(stdout, "gen-ts: cannot write the generated files: {e}");
            ExitCode::FAILURE
        }
    }
}

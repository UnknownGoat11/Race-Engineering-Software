//! Structured logging (docs/DECISIONS.md D-014).
//!
//! * JSON lines, one event per line, written through a non-blocking writer to a daily-rolling file in the
//!   per-user app log directory (Windows: `%LOCALAPPDATA%\com.lmurace.engineering\logs`), keeping
//!   [`MAX_LOG_FILES`] files.
//! * Every event emitted by this application carries a `component` field (see [`component`]).
//! * Debug builds additionally log human-readable lines to stderr.
//! * Filter: `LMURE_LOG` (EnvFilter syntax), default `info` (debug builds: our crate at `debug`).
//! * `log`-crate records from dependencies (Tauri, wry) are bridged into `tracing`.
//! * Secrets must never be logged; the diagnostics exporter applies a redaction pass as a second safety net.
//! * Logging failures never prevent the application from starting.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Values for the `component` field. Dotted, lowercase, stable (used for log filtering and diagnostics).
pub mod component {
    pub const APP: &str = "desktop.app";
    pub const CONFIG: &str = "desktop.config";
    pub const IPC: &str = "desktop.ipc";
    pub const PANIC: &str = "desktop.panic";
}

pub const LOG_FILE_PREFIX: &str = "lmu-race-engineering";
pub const LOG_FILE_SUFFIX: &str = "log";
pub const MAX_LOG_FILES: usize = 14;
pub const FILTER_ENV_VAR: &str = "LMURE_LOG";

/// Keeps the non-blocking file writer alive. Call [`LoggingGuard::flush_and_close`] on exit so buffered
/// lines are written before the process terminates.
#[derive(Default)]
pub struct LoggingGuard {
    worker: Mutex<Option<WorkerGuard>>,
}

impl LoggingGuard {
    pub fn flush_and_close(&self) {
        // Dropping the WorkerGuard flushes the queue and joins the writer thread.
        let guard = self
            .worker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(guard);
    }
}

pub struct LoggingInit {
    pub guard: LoggingGuard,
    /// Directory of the log files, if file logging is active.
    pub log_dir: Option<PathBuf>,
    /// Why file logging (or the subscriber) could not be set up, if it could not.
    pub error: Option<String>,
}

fn default_filter() -> EnvFilter {
    if cfg!(debug_assertions) {
        EnvFilter::new("info,lmu_race_engineering_lib=debug,lmu_race_engineering=debug")
    } else {
        EnvFilter::new("info")
    }
}

fn file_appender(dir: &Path) -> Result<RollingFileAppender, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("cannot create log directory {}: {e}", dir.display()))?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(dir)
        .map_err(|e| format!("cannot open log file in {}: {e}", dir.display()))
}

/// Install the global subscriber. `log_dir` is `None` if the path resolver could not provide one.
pub fn init(log_dir: Option<&Path>) -> LoggingInit {
    let filter = EnvFilter::try_from_env(FILTER_ENV_VAR).unwrap_or_else(|_| default_filter());

    let mut error = None;
    let mut worker = None;
    let mut active_dir = None;
    let file_layer = match log_dir.map(file_appender) {
        Some(Ok(appender)) => {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            worker = Some(guard);
            active_dir = log_dir.map(Path::to_path_buf);
            Some(
                fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_current_span(false)
                    .with_span_list(false)
                    .with_target(true)
                    .with_thread_names(true)
                    .with_ansi(false)
                    .with_writer(writer),
            )
        }
        Some(Err(message)) => {
            error = Some(message);
            None
        }
        None => {
            error = Some("the per-user log directory could not be resolved".to_owned());
            None
        }
    };

    let stderr_layer = cfg!(debug_assertions).then(|| {
        fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(false)
            .compact()
    });

    if let Err(e) = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
    {
        // Only possible if a global subscriber already exists (e.g. in tests).
        error = Some(format!(
            "a global tracing subscriber is already installed: {e}"
        ));
    }

    LoggingInit {
        guard: LoggingGuard {
            worker: Mutex::new(worker),
        },
        log_dir: active_dir,
        error,
    }
}

/// Log panics as structured events (the default hook still runs afterwards).
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        // (`PanicHookInfo::payload_as_str` would need Rust 1.91; the workspace MSRV is 1.90.)
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        tracing::error!(
            component = component::PANIC,
            location = location.as_deref(),
            "panic: {payload}"
        );
        default_hook(info);
    }));
}

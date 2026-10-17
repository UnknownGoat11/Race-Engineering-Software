//! The LMU reader thread (`"lmu-reader"`).
//!
//! Loops `LmuReader::next_snapshot(100ms)` and hands each `RawSnapshot` to the pipeline with `try_send` on a
//! bounded queue of 256 snapshots. A full queue drops the newest snapshot, counts `reader_overrun` and flags
//! an overrun so the pipeline marks a discontinuity. Publishes `ReaderDiagnostics` at 4 Hz through an
//! `ArcSwap`. Never touches the network, the recorder or the UI. Stop is an `AtomicBool` honoured within
//! 100 ms. LMU is only ever read (`FILE_MAP_READ` in `lmu-reader`).
//!
//! Implemented in app-core Phase 2 Increment I-3 (blueprint §2, §3, §4, §7, §12).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use crossbeam_channel::{Sender, TrySendError};
use lmu_reader::{LmuError, LmuReader, RawSnapshot, ReaderConfig, ReaderDiagnostics, ReaderStats};
use thiserror::Error;
use tracing::{debug, error, info, warn};

/// Tracing component name for logging.
const COMPONENT: &str = "app_core::driver::reader_thread";

/// Bounded queue capacity between the reader thread and the pipeline thread (blueprint §2, §7).
pub const READER_QUEUE_CAPACITY: usize = 256;

/// Default timeout for waiting for the next snapshot before checking the stop flag (100 ms).
pub const DEFAULT_SNAPSHOT_TIMEOUT: Duration = Duration::from_millis(100);

/// Interval for publishing reader diagnostics to `ArcSwap` (4 Hz = 250 ms, blueprint §2).
pub const DIAGNOSTICS_PUBLISH_INTERVAL: Duration = Duration::from_millis(250);

/// Errors originating from the reader thread supervisor.
#[derive(Debug, Error)]
pub enum ReaderThreadError {
    /// Failed to spawn the OS reader thread.
    #[error("failed to spawn reader thread: {0}")]
    Spawn(#[from] std::io::Error),

    /// Reader initialisation or capture error.
    #[error("reader error: {0}")]
    Lmu(#[from] LmuError),

    /// Reader thread panicked or exited with error.
    #[error("reader thread failed or panicked: {0}")]
    ThreadPanicked(String),
}

/// Message emitted from the reader thread across the bounded queue to the pipeline.
#[derive(Debug, Clone)]
pub struct ReaderSnapshotMessage {
    /// The acquired LMU raw snapshot.
    pub snapshot: RawSnapshot,
    /// `true` if at least one snapshot was dropped immediately prior to this message due to queue congestion.
    pub overrun_before: bool,
    /// Point-in-time reader throughput statistics, if polled.
    pub stats: Option<ReaderStats>,
}

/// Trait abstracting the underlying acquisition source for headless testing and capture replay.
pub trait SnapshotSource: Send + 'static {
    /// Waits up to `timeout` for the next snapshot. Returns `None` if timeout expires without a new frame.
    fn next_snapshot(&mut self, timeout: Duration) -> Result<Option<RawSnapshot>, LmuError>;

    /// Returns the current point-in-time diagnostics view.
    fn diagnostics(&self) -> ReaderDiagnostics;

    /// Returns the current point-in-time reader rate statistics.
    fn stats(&self) -> ReaderStats;

    /// Cleanly closes the acquisition source and unmaps shared memory.
    fn close(&mut self);
}

impl SnapshotSource for LmuReader {
    fn next_snapshot(&mut self, timeout: Duration) -> Result<Option<RawSnapshot>, LmuError> {
        self.next_snapshot(timeout)
    }

    fn diagnostics(&self) -> ReaderDiagnostics {
        self.diagnostics()
    }

    fn stats(&self) -> ReaderStats {
        self.stats().clone()
    }

    fn close(&mut self) {
        self.close();
    }
}

/// Configuration options for the reader thread.
#[derive(Debug, Clone)]
pub struct ReaderThreadConfig {
    /// Maximum duration to wait inside `next_snapshot` per iteration.
    pub snapshot_timeout: Duration,
    /// Interval at which `ReaderDiagnostics` are published to the shared `ArcSwap`.
    pub diagnostics_interval: Duration,
}

impl Default for ReaderThreadConfig {
    fn default() -> Self {
        Self {
            snapshot_timeout: DEFAULT_SNAPSHOT_TIMEOUT,
            diagnostics_interval: DIAGNOSTICS_PUBLISH_INTERVAL,
        }
    }
}

/// Handle to the running reader thread allowing lifecycle monitoring, diagnostics access, and clean shutdown.
pub struct ReaderThreadHandle {
    stop_flag: Arc<AtomicBool>,
    overruns: Arc<AtomicU64>,
    diagnostics: Arc<ArcSwap<Option<ReaderDiagnostics>>>,
    join_handle: Option<JoinHandle<Result<(), LmuError>>>,
}

impl ReaderThreadHandle {
    /// Signals the reader thread to stop. The thread will honour the signal within `snapshot_timeout` (<= 100 ms).
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
    }

    /// Checks if the reader thread is currently running.
    pub fn is_running(&self) -> bool {
        self.join_handle.as_ref().is_some_and(|h| !h.is_finished())
    }

    /// Total number of reader-to-pipeline queue overruns (dropped newest snapshots).
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    /// Current point-in-time reader diagnostics, if published.
    pub fn diagnostics(&self) -> Option<ReaderDiagnostics> {
        self.diagnostics.load().as_ref().clone()
    }

    /// Shared handle to the published diagnostics (read by the pipeline's housekeeping tick).
    pub fn diagnostics_source(&self) -> Arc<ArcSwap<Option<ReaderDiagnostics>>> {
        Arc::clone(&self.diagnostics)
    }

    /// Stops and joins the reader thread, returning any error encountered during acquisition.
    pub fn join(mut self) -> Result<(), ReaderThreadError> {
        self.stop();
        if let Some(handle) = self.join_handle.take() {
            match handle.join() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(err)) => Err(ReaderThreadError::Lmu(err)),
                Err(panic) => {
                    let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else if let Some(s) = panic.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    Err(ReaderThreadError::ThreadPanicked(msg))
                }
            }
        } else {
            Ok(())
        }
    }
}

/// Spawns the dedicated `"lmu-reader"` thread around the provided [`SnapshotSource`].
pub fn spawn_reader_thread(
    mut source: Box<dyn SnapshotSource>,
    config: ReaderThreadConfig,
    tx: Sender<ReaderSnapshotMessage>,
) -> Result<ReaderThreadHandle, ReaderThreadError> {
    let stop_flag = Arc::new(AtomicBool::new(false));
    let overruns = Arc::new(AtomicU64::new(0));
    let initial_diag = Some(source.diagnostics());
    let diagnostics = Arc::new(ArcSwap::new(Arc::new(initial_diag)));

    let thread_stop = Arc::clone(&stop_flag);
    let thread_overruns = Arc::clone(&overruns);
    let thread_diagnostics = Arc::clone(&diagnostics);

    let join_handle = Builder::new()
        .name("lmu-reader".to_string())
        .spawn(move || {
            info!(
                component = COMPONENT,
                "lmu-reader thread started (256 snapshot bound, 100ms timeout)"
            );

            let mut last_diag_publish = Instant::now();
            let mut pending_overrun = false;

            while !thread_stop.load(Ordering::Relaxed) {
                // Periodically update diagnostics through ArcSwap (4 Hz default):
                if last_diag_publish.elapsed() >= config.diagnostics_interval {
                    thread_diagnostics.store(Arc::new(Some(source.diagnostics())));
                    last_diag_publish = Instant::now();
                }

                // Acquire next snapshot (up to snapshot_timeout):
                match source.next_snapshot(config.snapshot_timeout) {
                    Ok(Some(snapshot)) => {
                        let msg = ReaderSnapshotMessage {
                            snapshot,
                            overrun_before: pending_overrun,
                            stats: Some(source.stats()),
                        };

                        match tx.try_send(msg) {
                            Ok(()) => {
                                pending_overrun = false;
                            }
                            Err(TrySendError::Full(_dropped)) => {
                                thread_overruns.fetch_add(1, Ordering::Relaxed);
                                pending_overrun = true;
                                warn!(
                                    component = COMPONENT,
                                    "reader-to-pipeline queue full (256 snapshots); dropped newest snapshot"
                                );
                            }
                            Err(TrySendError::Disconnected(_)) => {
                                debug!(
                                    component = COMPONENT,
                                    "pipeline receiver disconnected; reader thread terminating"
                                );
                                break;
                            }
                        }
                    }
                    Ok(None) => {
                        // Timeout expired with no new frame; loop back to check stop flag
                    }
                    Err(err) => {
                        error!(
                            component = COMPONENT,
                            error = %err,
                            "error acquiring snapshot from lmu reader"
                        );
                        // IncompatibleInterface or unrecoverable error:
                        thread_diagnostics.store(Arc::new(Some(source.diagnostics())));
                        source.close();
                        return Err(err);
                    }
                }
            }

            info!(
                component = COMPONENT,
                "lmu-reader thread stopping; closing acquisition source"
            );
            thread_diagnostics.store(Arc::new(Some(source.diagnostics())));
            source.close();
            Ok(())
        })?;

    Ok(ReaderThreadHandle {
        stop_flag,
        overruns,
        diagnostics,
        join_handle: Some(join_handle),
    })
}

/// Spawns the dedicated `"lmu-reader"` thread attaching directly to live LMU shared memory.
pub fn spawn_lmu_reader(
    reader_config: ReaderConfig,
    thread_config: ReaderThreadConfig,
    tx: Sender<ReaderSnapshotMessage>,
) -> Result<ReaderThreadHandle, ReaderThreadError> {
    let reader = LmuReader::open(reader_config)?;
    spawn_reader_thread(Box::new(reader), thread_config, tx)
}

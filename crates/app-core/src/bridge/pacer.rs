//! The 60 Hz flush loop (`"ui-bridge"` thread).
//!
//! Ticks periodically and flushes every subscription's pending blocks as one or more
//! `UiBatch`es. The 4 Hz `core:status` emitter runs on the same thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, Builder, JoinHandle};
use std::time::{Duration, Instant};

use tracing::{debug, error};

use crate::bridge::hub::UiHub;

/// Default target flush frequency for UI telemetry (60 Hz).
pub const DEFAULT_FLUSH_HZ: u32 = 60;

/// Default frequency for composite `core:status` JSON event emission (4 Hz).
pub const DEFAULT_STATUS_HZ: u32 = 4;

/// Configuration options for the bridge pacer loop.
#[derive(Debug, Clone)]
pub struct PacerConfig {
    pub flush_hz: u32,
    pub status_hz: u32,
}

impl Default for PacerConfig {
    fn default() -> Self {
        Self {
            flush_hz: DEFAULT_FLUSH_HZ,
            status_hz: DEFAULT_STATUS_HZ,
        }
    }
}

/// Periodic thread pacing UI deliveries at 60 Hz and status events at 4 Hz.
pub struct UiBridgePacer {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl UiBridgePacer {
    /// Spawns the `"ui-bridge"` thread.
    pub fn spawn<F>(hub: Arc<UiHub>, config: PacerConfig, status_emitter: Option<F>) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);

        let flush_hz = config.flush_hz.max(1);
        let status_hz = config.status_hz.max(1);
        let tick_duration = Duration::from_nanos(1_000_000_000 / flush_hz as u64);
        let status_interval_ticks = (flush_hz / status_hz).max(1);

        let thread_res = Builder::new().name("ui-bridge".to_string()).spawn(move || {
            debug!(component = "ui-bridge", "started ui-bridge pacer thread");
            let mut tick_counter: u32 = 0;
            let mut next_tick = Instant::now() + tick_duration;

            while running_clone.load(Ordering::Relaxed) {
                // 1. Flush all pending blocks in the hub:
                hub.flush_all();

                // 2. 4 Hz status event emission:
                tick_counter = tick_counter.saturating_add(1);
                if tick_counter >= status_interval_ticks {
                    tick_counter = 0;
                    if let Some(ref emit_fn) = status_emitter {
                        emit_fn();
                    }
                }

                // 3. Sleep until next tick:
                let now = Instant::now();
                if next_tick > now {
                    thread::sleep(next_tick - now);
                }
                next_tick = next_tick
                    .checked_add(tick_duration)
                    .unwrap_or_else(|| Instant::now() + tick_duration);
                if next_tick < Instant::now() {
                    next_tick = Instant::now() + tick_duration;
                }
            }
            debug!(component = "ui-bridge", "stopped ui-bridge pacer thread");
        });

        let thread = match thread_res {
            Ok(handle) => Some(handle),
            Err(err) => {
                error!(
                    component = "ui-bridge",
                    error = %err,
                    "failed to spawn ui-bridge pacer thread"
                );
                None
            }
        };

        Self { running, thread }
    }

    /// Signals the pacer thread to stop and waits for it to exit.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for UiBridgePacer {
    fn drop(&mut self) {
        self.stop();
    }
}

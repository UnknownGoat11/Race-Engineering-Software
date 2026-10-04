//! The reader runs on its own OS thread (as it will in the app) and hands frames and periodic
//! status to the UI thread through a bounded channel. If the UI falls behind, frames are dropped
//! and counted, never buffered without bound: the channel is bounded by message count and the raw
//! capture blocks in flight are additionally bounded by bytes.
//!
//! The thread is always joined (also when the worker is dropped on an early error return), so the
//! process never exits while the reader holds LMU's lock.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use lmu_reader::{
    CompatibilityReport, LmuReader, OwnedBlocks, RawSnapshot, ReaderConfig, ReaderDiagnostics,
    ReaderStats,
};

use crate::stop::STOP;
use crate::{COMPONENT, DiagError};

/// Capacity of the reader → UI channel (≈ 40 s of telemetry at 100 Hz).
const CHANNEL_CAPACITY: usize = 4_096;
/// Upper bound of raw capture bytes queued but not yet received (≈ 6 s of a 30-car field at
/// 100 Hz; a full 104-car frame is about 260 KiB).
pub(crate) const MAX_RAW_BYTES_IN_FLIGHT: usize = 64 << 20;
/// Bound of each `next_snapshot` call (how quickly a stop request is noticed).
const READER_POLL: Duration = Duration::from_millis(50);

/// One new frame.
pub(crate) struct Frame {
    /// The decoded frame.
    pub(crate) snapshot: RawSnapshot,
    /// The present raw blocks behind it, trimmed (only when requested, for captures).
    pub(crate) raw: Option<OwnedBlocks>,
}

/// Periodic reader status.
#[derive(Clone)]
pub(crate) struct Status {
    pub(crate) diagnostics: ReaderDiagnostics,
    pub(crate) stats: ReaderStats,
    pub(crate) compat: CompatibilityReport,
    /// Messages dropped because the channel (or the raw byte budget) was full.
    pub(crate) dropped: u64,
}

/// Reader thread → UI thread messages.
pub(crate) enum Msg {
    Frame(Box<Frame>),
    Status(Box<Status>),
    /// A reader error (repeats of the same text are suppressed).
    Error(String),
}

/// Handle to the reader thread.
pub(crate) struct Worker {
    rx: Receiver<Msg>,
    stop: Arc<AtomicBool>,
    raw_in_flight: Arc<AtomicUsize>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    /// Opens the reader and starts its thread.
    pub(crate) fn spawn(
        config: ReaderConfig,
        keep_raw: bool,
        status_every: Duration,
    ) -> Result<Self, DiagError> {
        let activity = STOP.reader_activity();
        let reader = LmuReader::open(config)?;
        let (tx, rx) = sync_channel(CHANNEL_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let raw_in_flight = Arc::new(AtomicUsize::new(0));
        let ctx = ThreadCtx {
            tx,
            stop: Arc::clone(&stop),
            raw_in_flight: Arc::clone(&raw_in_flight),
            keep_raw,
            status_every,
        };
        let handle = std::thread::Builder::new()
            .name("lmu-reader".to_owned())
            .spawn(move || {
                // Counted until the reader is closed and dropped (the console-close handler
                // waits for this).
                let _activity = activity;
                run(reader, &ctx);
            })
            .map_err(|e| DiagError::Worker(format!("cannot start: {e}")))?;
        Ok(Self {
            rx,
            stop,
            raw_in_flight,
            handle: Some(handle),
        })
    }

    /// Waits for the next message.
    pub(crate) fn recv_timeout(&self, timeout: Duration) -> Result<Msg, RecvTimeoutError> {
        let msg = self.rx.recv_timeout(timeout)?;
        if let Msg::Frame(frame) = &msg
            && let Some(raw) = &frame.raw
        {
            // The receiver owns it now: release its share of the byte budget.
            self.raw_in_flight
                .fetch_sub(raw.byte_len(), Ordering::SeqCst);
        }
        Ok(msg)
    }

    /// Stops the thread and waits for it.
    pub(crate) fn stop(mut self) -> Result<(), DiagError> {
        self.join()
    }

    fn join(&mut self) -> Result<(), DiagError> {
        self.stop.store(true, Ordering::SeqCst);
        match self.handle.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| DiagError::Worker("the reader thread panicked".to_owned())),
            None => Ok(()),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Early `?` returns end up here: never leave the thread running (it could be inside LMU's
        // lock when the process exits).
        if let Err(e) = self.join() {
            tracing::warn!(component = COMPONENT, error = %e, "reader thread ended abnormally");
        }
    }
}

/// What the reader thread owns besides the reader.
struct ThreadCtx {
    tx: SyncSender<Msg>,
    stop: Arc<AtomicBool>,
    raw_in_flight: Arc<AtomicUsize>,
    keep_raw: bool,
    status_every: Duration,
}

impl ThreadCtx {
    fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || STOP.is_requested()
    }
}

fn send(tx: &SyncSender<Msg>, msg: Msg, dropped: &mut u64) -> bool {
    match tx.try_send(msg) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            *dropped += 1;
            true
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

/// Reserves `bytes` of the raw budget; `false` (frame dropped) if it would be exceeded.
fn reserve(in_flight: &AtomicUsize, bytes: usize) -> bool {
    // Explicit CAS loop (`fetch_update` is deprecated on newer toolchains).
    let mut current = in_flight.load(Ordering::SeqCst);
    loop {
        let Some(total) = current
            .checked_add(bytes)
            .filter(|&total| total <= MAX_RAW_BYTES_IN_FLIGHT)
        else {
            return false;
        };
        match in_flight.compare_exchange_weak(current, total, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}

/// Sends a frame, accounting its raw bytes against the budget.
fn send_frame(ctx: &ThreadCtx, frame: Frame, dropped: &mut u64) -> bool {
    let bytes = frame.raw.as_ref().map_or(0, OwnedBlocks::byte_len);
    if !reserve(&ctx.raw_in_flight, bytes) {
        *dropped += 1;
        return true;
    }
    match ctx.tx.try_send(Msg::Frame(Box::new(frame))) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            ctx.raw_in_flight.fetch_sub(bytes, Ordering::SeqCst);
            *dropped += 1;
            true
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

fn run(mut reader: LmuReader, ctx: &ThreadCtx) {
    let mut dropped = 0u64;
    let mut last_error: Option<String> = None;
    let mut next_status = Instant::now();
    while !ctx.stopping() {
        let delivered = match reader.next_snapshot(READER_POLL) {
            Ok(Some(snapshot)) => {
                last_error = None;
                let raw = if ctx.keep_raw {
                    reader.last_raw_blocks().map(|b| b.to_owned_blocks())
                } else {
                    None
                };
                send_frame(ctx, Frame { snapshot, raw }, &mut dropped)
            }
            Ok(None) => true,
            Err(e) => {
                let text = e.to_string();
                if last_error.as_deref() == Some(text.as_str()) {
                    true
                } else {
                    last_error = Some(text.clone());
                    send(&ctx.tx, Msg::Error(text), &mut dropped)
                }
            }
        };
        if !delivered {
            break; // the UI thread is gone
        }
        if Instant::now() >= next_status {
            next_status = Instant::now() + ctx.status_every;
            let status = Status {
                diagnostics: reader.diagnostics(),
                stats: reader.stats().clone(),
                compat: reader.compatibility(),
                dropped,
            };
            if !send(&ctx.tx, Msg::Status(Box::new(status)), &mut dropped) {
                break;
            }
        }
    }
    reader.close();
    tracing::debug!(component = COMPONENT, dropped, "reader thread stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use lmu_reader::ObjectNames;

    /// Object names no LMU or other test ever creates: the reader just waits.
    const ABSENT: ObjectNames = ObjectNames {
        data_mapping: "lmu-diag-test-absent-data",
        hold_event: "lmu-diag-test-absent-hold",
        data_event: "lmu-diag-test-absent-data-event",
        lock_mapping: "lmu-diag-test-absent-lock",
        lock_event: "lmu-diag-test-absent-lock-event",
    };

    #[test]
    fn dropping_the_worker_joins_the_reader_thread() {
        let config = ReaderConfig {
            object_names: ABSENT,
            ..ReaderConfig::default()
        };
        // Only this test uses the global STOP state.
        assert_eq!(STOP.active_readers(), 0);
        let Ok(worker) = Worker::spawn(config, false, Duration::from_millis(10)) else {
            return; // not Windows: the reader is unsupported
        };
        assert_eq!(STOP.active_readers(), 1);
        // Simulates an early `?` return: the worker is dropped, not stopped explicitly.
        drop(worker);
        assert_eq!(
            STOP.active_readers(),
            0,
            "drop returned only after the reader thread closed the reader and ended"
        );
    }

    #[test]
    fn raw_bytes_in_flight_are_bounded() {
        let in_flight = AtomicUsize::new(0);
        assert!(reserve(&in_flight, MAX_RAW_BYTES_IN_FLIGHT - 10));
        assert!(!reserve(&in_flight, 11), "budget exceeded: dropped");
        assert!(reserve(&in_flight, 10));
        assert_eq!(in_flight.load(Ordering::SeqCst), MAX_RAW_BYTES_IN_FLIGHT);
        assert!(!reserve(&in_flight, usize::MAX), "no overflow");
    }
}

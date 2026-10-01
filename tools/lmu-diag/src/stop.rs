//! Clean shutdown on Ctrl+C, Ctrl+Break and console close / logoff / shutdown.
//!
//! Without a handler Windows runs the default one, which calls `ExitProcess` and kills the reader
//! thread wherever it is, including inside the SDK lock: `busy` would then stay 1 until LMU
//! restarts (research §5.4, A-LMU-052) and LMU's own publisher would fail every frame. The handler
//! below only requests a stop; the commands notice it, stop the reader (which releases the lock
//! after the copy in progress) and return normally. For a console close Windows terminates the
//! process as soon as the handler returns, so the handler waits (bounded) until no reader is active.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How long a close / logoff / shutdown event waits for the readers to stop (Windows allows 5 s
/// for `CTRL_CLOSE_EVENT`; a reader stops within one `next_snapshot` bound, about 100 ms).
const CLOSE_GRACE: Duration = Duration::from_secs(4);
/// Poll interval while waiting for the readers to stop.
const RELEASE_POLL: Duration = Duration::from_millis(10);

/// Console control events (`wincon.h`).
mod event {
    pub(super) const CTRL_C: u32 = 0;
    pub(super) const CTRL_BREAK: u32 = 1;
    pub(super) const CTRL_CLOSE: u32 = 2;
    pub(super) const CTRL_LOGOFF: u32 = 5;
    pub(super) const CTRL_SHUTDOWN: u32 = 6;
}

/// Stop request plus the number of live `LmuReader`s.
#[derive(Debug, Default)]
pub(crate) struct StopState {
    requested: AtomicBool,
    active_readers: AtomicUsize,
}

/// The process-wide instance. A console control handler receives no context pointer, so this is
/// the one piece of static state of the tool; it only holds atomics and is never reset.
pub(crate) static STOP: StopState = StopState::new();

impl StopState {
    pub(crate) const fn new() -> Self {
        Self {
            requested: AtomicBool::new(false),
            active_readers: AtomicUsize::new(0),
        }
    }

    /// Asks every command to stop.
    pub(crate) fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
    }

    /// `true` once a stop was requested.
    pub(crate) fn is_requested(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Marks a reader as alive until the returned guard is dropped (after the reader is closed).
    pub(crate) fn reader_activity(&self) -> ReaderActivity<'_> {
        self.active_readers.fetch_add(1, Ordering::SeqCst);
        ReaderActivity(self)
    }

    /// Number of readers alive.
    pub(crate) fn active_readers(&self) -> usize {
        self.active_readers.load(Ordering::SeqCst)
    }

    /// Waits until no reader is alive or `timeout` passed; `true` if none is alive.
    fn wait_released(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.active_readers() > 0 {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(RELEASE_POLL);
        }
        true
    }

    /// Handles one console control event; `true` if it was handled (the default handler, which
    /// kills the process immediately, must then not run).
    pub(crate) fn on_console_event(&self, ctrl_type: u32) -> bool {
        match ctrl_type {
            event::CTRL_C | event::CTRL_BREAK => {
                self.request();
                true
            }
            event::CTRL_CLOSE | event::CTRL_LOGOFF | event::CTRL_SHUTDOWN => {
                self.request();
                // The process ends when this returns: let the readers release the lock first.
                self.wait_released(CLOSE_GRACE);
                true
            }
            _ => false,
        }
    }
}

/// Keeps [`StopState::active_readers`] incremented while a reader exists.
#[derive(Debug)]
pub(crate) struct ReaderActivity<'a>(&'a StopState);

impl Drop for ReaderActivity<'_> {
    fn drop(&mut self) {
        self.0.active_readers.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Installs the console control handler.
///
/// # Errors
/// The OS error text if the handler cannot be registered.
#[cfg(windows)]
pub(crate) fn install() -> Result<(), String> {
    use windows::Win32::System::Console::SetConsoleCtrlHandler;
    use windows::core::BOOL;

    unsafe extern "system" fn handler(ctrl_type: u32) -> BOOL {
        BOOL::from(STOP.on_console_event(ctrl_type))
    }

    // SAFETY: registers a function with 'static lifetime that only touches atomics in a static
    // (`STOP`) and sleeps; it is safe to run on the console's control thread at any time.
    unsafe { SetConsoleCtrlHandler(Some(handler), true) }.map_err(|e| e.to_string())
}

/// Installs nothing on other platforms (the reader is Windows-only).
#[cfg(not(windows))]
pub(crate) fn install() -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_c_requests_a_stop_and_is_handled() {
        let state = StopState::new();
        assert!(!state.is_requested());
        assert!(state.on_console_event(event::CTRL_C));
        assert!(state.is_requested());
        let state = StopState::new();
        assert!(state.on_console_event(event::CTRL_BREAK));
        assert!(state.is_requested());
        assert!(!StopState::new().on_console_event(42));
    }

    #[test]
    fn close_waits_for_active_readers_to_stop() {
        let state = std::sync::Arc::new(StopState::new());
        let activity_state = std::sync::Arc::clone(&state);
        let reader = std::thread::spawn(move || {
            let _activity = activity_state.reader_activity();
            while !activity_state.is_requested() {
                std::thread::sleep(RELEASE_POLL);
            }
            std::thread::sleep(Duration::from_millis(50)); // finishing the copy in progress
        });
        while state.active_readers() == 0 {
            std::thread::sleep(RELEASE_POLL);
        }
        let started = Instant::now();
        assert!(state.on_console_event(event::CTRL_CLOSE));
        assert_eq!(
            state.active_readers(),
            0,
            "returned only after the reader stopped"
        );
        assert!(started.elapsed() < CLOSE_GRACE);
        reader.join().expect("reader thread");
    }
}

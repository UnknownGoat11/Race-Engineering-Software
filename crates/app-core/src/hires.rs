//! `HiResTimer`: the high-resolution periodic timer behind the 60 Hz UI flush.
//!
//! Uses `CreateWaitableTimerExW` with `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` (`windows` crate,
//! `Win32_System_Threading`), so ticks do not alternate on the 15.6 ms coarse system timer. Fallback when
//! high resolution is unavailable: a normal waitable timer plus a spin over the last 2 ms. Target: over a
//! 5 s run, at least 98 % of ticks within +/-2 ms.
//!
//! This is the only module in app-core allowed to contain unsafe code: every unsafe block keeps
//! `unsafe_op_in_unsafe_fn` and an explicit SAFETY comment explaining invariant guarantees.

#![allow(unsafe_code)]

use std::time::{Duration, Instant};
use thiserror::Error;

/// Error returned by high-resolution timer operations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HiResError {
    /// Specified frequency is invalid (must be positive and finite).
    #[error("invalid timer frequency {0} Hz (must be positive and finite)")]
    InvalidFrequency(String),

    /// OS timer creation failed.
    #[error("failed to create OS waitable timer: {0}")]
    OsError(String),
}

#[cfg(windows)]
mod win32 {
    use super::*;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, SetWaitableTimer,
        TIMER_ALL_ACCESS, WaitForSingleObject,
    };
    use windows::core::PCWSTR;

    pub struct WinWaitableTimer {
        handle: HANDLE,
        is_high_res: bool,
    }

    impl WinWaitableTimer {
        pub fn new() -> Result<Self, HiResError> {
            // Try CREATE_WAITABLE_TIMER_HIGH_RESOLUTION first.
            // SAFETY: lpTimerAttributes is null, lpTimerName is null (unnamed timer),
            // CREATE_WAITABLE_TIMER_HIGH_RESOLUTION requests a high-resolution timer handle.
            let (handle, is_high_res) = unsafe {
                let res = CreateWaitableTimerExW(
                    None,
                    PCWSTR::null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS.0,
                );
                match res {
                    Ok(h) => (h, true),
                    Err(_) => {
                        // Fallback to standard timer (flags = 0)
                        let fallback =
                            CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0);
                        let h = fallback.map_err(|e| HiResError::OsError(e.to_string()))?;
                        (h, false)
                    }
                }
            };

            Ok(Self {
                handle,
                is_high_res,
            })
        }

        pub fn is_high_res(&self) -> bool {
            self.is_high_res
        }

        pub fn wait_relative(&self, duration: Duration) -> Result<(), HiResError> {
            // SetWaitableTimer due time is in 100-nanosecond units.
            // Negative values indicate relative time.
            let nanos = duration.as_nanos();
            let units_100ns = (nanos / 100).min(i64::MAX as u128) as i64;
            let due_time = -units_100ns;

            // SAFETY: `self.handle` is a valid waitable timer handle. `due_time` is a valid i64.
            // Period is 0 (one-shot), completion routine is None.
            unsafe {
                SetWaitableTimer(self.handle, &due_time, 0, None, None, false)
                    .map_err(|e| HiResError::OsError(e.to_string()))?;
            }

            // Timeout in ms: duration + 1000ms safety buffer
            let timeout_ms = (duration.as_millis() as u32).saturating_add(1000);

            // SAFETY: `self.handle` is valid. Waiting until signaled or timeout.
            let wait_res = unsafe { WaitForSingleObject(self.handle, timeout_ms) };
            if wait_res == WAIT_OBJECT_0 || wait_res == WAIT_TIMEOUT {
                Ok(())
            } else {
                Err(HiResError::OsError(format!(
                    "WaitForSingleObject returned {wait_res:?}"
                )))
            }
        }
    }

    impl Drop for WinWaitableTimer {
        fn drop(&mut self) {
            if !self.handle.is_invalid() {
                // SAFETY: self.handle is a valid owned Win32 handle.
                unsafe {
                    let _ = CloseHandle(self.handle);
                }
            }
        }
    }
}

/// Periodic high-resolution timer.
pub struct HiResTimer {
    frequency_hz: f64,
    period: Duration,
    start_time: Instant,
    ticks_completed: u64,
    #[cfg(windows)]
    os_timer: Option<win32::WinWaitableTimer>,
}

impl HiResTimer {
    /// Creates a new periodic timer with the specified frequency in Hz.
    pub fn new(frequency_hz: f64) -> Result<Self, HiResError> {
        if !frequency_hz.is_finite() || frequency_hz <= 0.0 {
            return Err(HiResError::InvalidFrequency(frequency_hz.to_string()));
        }

        let period_nanos = (1_000_000_000.0 / frequency_hz).round() as u64;
        let period = Duration::from_nanos(period_nanos.max(1));

        #[cfg(windows)]
        let os_timer = win32::WinWaitableTimer::new().ok();

        Ok(Self {
            frequency_hz,
            period,
            start_time: Instant::now(),
            ticks_completed: 0,
            #[cfg(windows)]
            os_timer,
        })
    }

    /// The configured target frequency in Hz.
    pub fn frequency_hz(&self) -> f64 {
        self.frequency_hz
    }

    /// The nominal period between ticks.
    pub fn period(&self) -> Duration {
        self.period
    }

    /// Whether this timer is backed by an OS high-resolution timer (e.g. Windows high-res waitable timer).
    pub fn is_high_resolution(&self) -> bool {
        #[cfg(windows)]
        {
            self.os_timer.as_ref().is_some_and(|t| t.is_high_res())
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// Resets the anchor start time and completed ticks counter to `now`.
    pub fn reset(&mut self) {
        self.start_time = Instant::now();
        self.ticks_completed = 0;
    }

    /// Waits until the next scheduled tick and returns the current instant.
    ///
    /// The pacing anchor prevents long-term drift: tick `k` targets `start_time + k * period`.
    /// When behind schedule (e.g. after a pause), it catches up to the current wall time without
    /// emitting rapid bursts.
    pub fn tick(&mut self) -> Result<Instant, HiResError> {
        let next_tick = self.ticks_completed.saturating_add(1);
        let target_nanos =
            (next_tick as f64 * (1_000_000_000.0 / self.frequency_hz)).round() as u64;
        let target = self.start_time + Duration::from_nanos(target_nanos);

        let now = Instant::now();
        if now < target {
            let remaining = target - now;

            let spin_margin = if cfg!(windows) {
                Duration::from_micros(1500)
            } else {
                Duration::from_millis(2)
            };

            #[cfg(windows)]
            if let Some(ref os_timer) = self.os_timer {
                if remaining > spin_margin {
                    let _ = os_timer.wait_relative(remaining - spin_margin);
                }
            } else if remaining > spin_margin {
                std::thread::sleep(remaining - spin_margin);
            }

            #[cfg(not(windows))]
            {
                let spin_margin = Duration::from_millis(2);
                if remaining > spin_margin {
                    std::thread::sleep(remaining - spin_margin);
                }
            }

            // Final microsecond spin to eliminate any residual scheduling jitter.
            while Instant::now() < target {
                std::hint::spin_loop();
            }
        } else if now > target + self.period {
            // Overrun: advance ticks_completed to re-synchronize with current time
            let overdue = now - target;
            let missed_ticks = (overdue.as_nanos() / self.period.as_nanos()) as u64;
            self.ticks_completed = self.ticks_completed.saturating_add(missed_ticks);
        }

        self.ticks_completed = self.ticks_completed.saturating_add(1);
        Ok(Instant::now())
    }
}

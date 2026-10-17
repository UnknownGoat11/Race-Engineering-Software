//! CPU time of the calling thread: the recorder reports its own cost (CPU seconds per recorded hour) in
//! the session summary.

use std::time::Duration;

/// User plus kernel CPU time consumed so far by the calling thread, if the platform reports it.
#[cfg(windows)]
pub(crate) fn current_thread_cpu_time() -> Option<Duration> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

    /// FILETIME counts 100 ns intervals.
    const FILETIME_TICK_NS: u64 = 100;

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: GetCurrentThread returns a pseudo-handle for the calling thread that is always valid and needs
    // no closing; the four out-pointers point to live, writable FILETIME locals for the whole call, and the
    // function writes nothing else.
    unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    let total = ticks(kernel).saturating_add(ticks(user));
    Some(Duration::from_nanos(total.saturating_mul(FILETIME_TICK_NS)))
}

/// User plus kernel CPU time consumed so far by the calling thread, if the platform reports it.
#[cfg(not(windows))]
pub(crate) fn current_thread_cpu_time() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn cpu_time_grows_with_work() {
        // Windows charges thread CPU time per scheduler tick (~15.6 ms), so spin until it advances rather
        // than for a fixed wall time (a loaded machine may run this thread for less than a tick).
        const GIVE_UP_AFTER: Duration = Duration::from_secs(30);
        let before = current_thread_cpu_time().expect("supported on Windows");
        let mut x = 0u64;
        let start = std::time::Instant::now();
        loop {
            for _ in 0..100_000 {
                x = std::hint::black_box(x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1));
            }
            let now = current_thread_cpu_time().expect("supported on Windows");
            if now > before {
                break;
            }
            assert!(
                start.elapsed() < GIVE_UP_AFTER,
                "thread CPU time never advanced from {before:?}"
            );
        }
    }
}

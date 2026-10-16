//! `HiResTimer`: the high-resolution periodic timer behind the 60 Hz UI flush.
//!
//! Uses `CreateWaitableTimerExW` with `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` (`windows` crate,
//! `Win32_System_Threading`), so ticks do not alternate on the 15.6 ms coarse system timer. Fallback when
//! high resolution is unavailable: a normal waitable timer plus a spin over the last 2 ms. Target: over a
//! 5 s run, at least 98 % of ticks within +/-2 ms.
//!
//! This is the only module in app-core allowed to contain unsafe code: the crate denies `unsafe_code` and
//! this module will opt out locally. Every unsafe block keeps `unsafe_op_in_unsafe_fn` and a SAFETY comment.
//!
//! Stub: implemented in Phase 1 lane B.

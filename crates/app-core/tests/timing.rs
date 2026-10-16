//! Integration and timing verification tests for `app-core` (Phase 1 Lane B).
//!
//! Covers:
//! - `AppClock` monotonic behavior, epoch preservation, and recorder `CaptureClock` trait implementation.
//! - `SkewTracker` wall-versus-monotonic drift detection (>5 s discontinuity threshold).
//! - `HiResTimer` Windows waitable timer precision (60 Hz target, >=98% within +/-2 ms jitter).

use std::time::{Duration, Instant, SystemTime};

use app_core::clock::{AppClock, SKEW_DISCONTINUITY_THRESHOLD, SkewStatus, SkewTracker};
use app_core::hires::{HiResError, HiResTimer};
use recorder::CaptureClock;

#[test]
fn app_clock_monotonic_and_capture_clock() {
    let clock = AppClock::new();
    let t0 = clock.now_ns();
    std::thread::sleep(Duration::from_millis(5));
    let t1 = clock.now_ns();
    assert!(t1 > t0, "AppClock now_ns must advance monotonically");

    // Trait implementation for recorder:
    let cap_ns = CaptureClock::now_ns(&clock);
    assert!(
        cap_ns >= t1,
        "CaptureClock::now_ns must be >= previous now_ns"
    );

    let explicit_epoch = Instant::now();
    let clock_custom = AppClock::with_epoch(explicit_epoch);
    assert_eq!(clock_custom.epoch(), explicit_epoch);
}

#[test]
fn skew_tracker_normal_progression() {
    let mut tracker = SkewTracker::new();
    let status = tracker.check();
    assert!(matches!(status, SkewStatus::Synchronized { .. }));

    // Minor progression (100 ms wall and mono):
    let now_wall = SystemTime::now();
    let now_mono = Instant::now();
    let status_init = tracker.check_at(now_wall, now_mono);
    assert!(matches!(status_init, SkewStatus::Synchronized { .. }));

    let status_step = tracker.check_at(
        now_wall + Duration::from_millis(100),
        now_mono + Duration::from_millis(100),
    );
    assert!(matches!(status_step, SkewStatus::Synchronized { .. }));
}

#[test]
fn skew_tracker_detects_sleep_resume_jump() {
    let mut tracker = SkewTracker::new();
    let now_wall = SystemTime::now();
    let now_mono = Instant::now();
    tracker.check_at(now_wall, now_mono);

    // Simulate system sleep: wall clock jumped forward 10 seconds, but monotonic only 50 ms
    let sleep_wall = now_wall + Duration::from_secs(10);
    let sleep_mono = now_mono + Duration::from_millis(50);
    let status = tracker.check_at(sleep_wall, sleep_mono);

    match status {
        SkewStatus::Discontinuity {
            skew,
            wall_elapsed,
            mono_elapsed,
        } => {
            assert!(skew >= SKEW_DISCONTINUITY_THRESHOLD);
            assert!(wall_elapsed >= Duration::from_secs(10));
            assert!(mono_elapsed <= Duration::from_millis(100));
        }
        SkewStatus::Synchronized { .. } => {
            panic!("expected Discontinuity when wall clock jumped 10s")
        }
    }
}

#[test]
fn skew_tracker_detects_backward_clock_jump() {
    let mut tracker = SkewTracker::new();
    let now_wall = SystemTime::now();
    let now_mono = Instant::now();
    tracker.check_at(now_wall, now_mono);

    // Simulate NTP stepping backward by 6 seconds
    let ntp_wall = now_wall - Duration::from_secs(6);
    let ntp_mono = now_mono + Duration::from_millis(10);
    let status = tracker.check_at(ntp_wall, ntp_mono);

    match status {
        SkewStatus::Discontinuity { skew, .. } => {
            assert!(skew >= SKEW_DISCONTINUITY_THRESHOLD);
        }
        SkewStatus::Synchronized { .. } => {
            panic!("expected Discontinuity when wall clock stepped backward")
        }
    }
}

#[test]
fn hires_timer_rejects_invalid_frequencies() {
    assert!(matches!(
        HiResTimer::new(0.0),
        Err(HiResError::InvalidFrequency(_))
    ));
    assert!(matches!(
        HiResTimer::new(-60.0),
        Err(HiResError::InvalidFrequency(_))
    ));
    assert!(matches!(
        HiResTimer::new(f64::NAN),
        Err(HiResError::InvalidFrequency(_))
    ));
    assert!(matches!(
        HiResTimer::new(f64::INFINITY),
        Err(HiResError::InvalidFrequency(_))
    ));
}

#[test]
#[cfg(windows)]
fn hires_timer_60hz_jitter_windows() {
    let mut timer = HiResTimer::new(60.0).expect("failed to create 60 Hz HiResTimer");
    assert_eq!(timer.frequency_hz(), 60.0);
    let nominal_period = timer.period();
    assert_eq!(nominal_period.as_millis(), 16);

    // Warm-up tick
    let mut last_tick = timer.tick().expect("first tick must succeed");

    // Collect 300 ticks (~5.0 seconds at 60 Hz)
    let total_ticks = 300;
    let mut within_tolerance = 0;
    let tolerance = Duration::from_millis(2);

    for _ in 0..total_ticks {
        let tick_time = timer.tick().expect("tick must succeed");
        let dt = tick_time.duration_since(last_tick);
        last_tick = tick_time;

        let diff = dt.abs_diff(nominal_period);
        if diff <= tolerance {
            within_tolerance += 1;
        }
    }

    let pass_ratio = within_tolerance as f64 / total_ticks as f64;
    eprintln!(
        "HiResTimer 60Hz: {within_tolerance}/{total_ticks} ({:.1}%) ticks within +/-2ms tolerance (high-res: {})",
        pass_ratio * 100.0,
        timer.is_high_resolution()
    );

    // Blueprint requirement §4: over a 5 s run, at least 98 % of ticks within +/-2 ms.
    assert!(
        pass_ratio >= 0.98,
        "expected >= 98% of ticks within +/-2ms, got {:.1}% ({within_tolerance}/{total_ticks})",
        pass_ratio * 100.0
    );
}

#[test]
fn hires_timer_reset() {
    let mut timer = HiResTimer::new(100.0).expect("failed to create 100 Hz timer");
    let _ = timer.tick().expect("tick");
    timer.reset();
    let t0 = Instant::now();
    let tick = timer.tick().expect("tick after reset");
    assert!(tick >= t0);
}

#[test]
fn app_clock_unix_timestamps() {
    let clock = AppClock::new();
    let s = clock.now_unix_s();
    let ns = clock.now_unix_ns();

    // Sanity check: epoch should be past 2024 (> 1_700_000_000s)
    assert!(s > 1_700_000_000, "unix timestamp s={s} is implausible");
    assert!(
        ns > 1_700_000_000_000_000_000,
        "unix timestamp ns={ns} is implausible"
    );
    assert_eq!(s, ns / 1_000_000_000);
}

#[test]
fn pacer_with_hires_timer() {
    use app_core::bridge::hub::UiHub;
    use app_core::bridge::pacer::{PacerConfig, UiBridgePacer};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    let hub = Arc::new(UiHub::new());
    let status_emits = Arc::new(AtomicU32::new(0));
    let status_emits_clone = Arc::clone(&status_emits);

    let config = PacerConfig {
        flush_hz: 60,
        status_hz: 10,
    };

    let mut pacer = UiBridgePacer::spawn(
        hub,
        config,
        Some(move || {
            status_emits_clone.fetch_add(1, Ordering::SeqCst);
        }),
    );

    // Let it run for 150 ms (~9 flushes, ~1-2 status emits)
    std::thread::sleep(Duration::from_millis(150));
    pacer.stop();

    let emits = status_emits.load(Ordering::SeqCst);
    assert!(
        emits >= 1,
        "status emitter should have fired at least once (got {emits})"
    );
}

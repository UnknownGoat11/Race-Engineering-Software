//! Fuel engine tests (spec §289). Synthetic consumption is generated in-test.

use super::*;

const EPS: f64 = 1e-9;
const SAMPLES_PER_LAP: usize = 10;

fn info(id: u32) -> FuelLapInfo {
    FuelLapInfo {
        lap_id: LapId(id),
        lap_number: i32::try_from(id).ok(),
        lap_time_s: Some(100.0),
        complete: true,
        pit_in: false,
        pit_out: false,
        formation: false,
        caution: false,
        invalidated: false,
    }
}

struct Run {
    eng: FuelEngine,
    et: f64,
    fuel: f64,
    next: u32,
}

impl Run {
    /// Engine with `fuel` litres, positioned on a lap boundary.
    fn new(config: FuelConfig, fuel: f64) -> Self {
        let mut eng = FuelEngine::new(config);
        eng.update(&FuelSample {
            et_s: 0.0,
            fuel_l: Some(fuel),
            capacity_l: Some(100.0),
        });
        // The lap in progress when fuel was first seen has unknown consumption.
        let partial = eng.lap_completed(FuelLapInfo {
            complete: false,
            ..info(0)
        });
        assert_eq!(partial.exclusion, Some(ExclusionReason::NoFuelData));
        Self {
            eng,
            et: 0.0,
            fuel,
            next: 1,
        }
    }

    fn sample(&mut self, fuel: Option<f64>) {
        self.et += 10.0;
        self.eng.update(&FuelSample {
            et_s: self.et,
            fuel_l: fuel,
            capacity_l: Some(100.0),
        });
    }

    /// One lap consuming `used` litres evenly; `edit` adjusts the lap facts.
    fn lap(&mut self, used: f64, edit: impl FnOnce(&mut FuelLapInfo)) -> FuelLap {
        for _ in 0..SAMPLES_PER_LAP {
            self.fuel -= used / SAMPLES_PER_LAP as f64;
            let f = self.fuel;
            self.sample(Some(f));
        }
        let mut i = info(self.next);
        self.next += 1;
        edit(&mut i);
        self.eng.lap_completed(i)
    }

    fn laps(&mut self, n: usize, used: f64) {
        for _ in 0..n {
            self.lap(used, |_| {});
        }
    }
}

#[test]
fn forty_litres_at_four_per_lap_is_ten_laps() {
    let mut r = Run::new(FuelConfig::default(), 60.0);
    r.laps(5, 4.0);
    let e = r.eng.estimate(&FuelContext::default());
    assert!((e.current_fuel.value.expect("fuel") - 40.0).abs() < EPS);
    assert!((e.fuel_per_lap.value.expect("fpl") - 4.0).abs() < EPS);
    assert!((e.laps_remaining.value.expect("laps") - 10.0).abs() < EPS);
    assert_eq!(e.sample_count, 5);
    assert_eq!(e.confidence, Some(Confidence::High));
    assert_eq!(
        e.fuel_per_lap.provenance,
        telemetry_model::Provenance::Estimated
    );
    assert!((e.time_remaining.value.expect("time") - 1_000.0).abs() < 1e-6);
    assert!((e.fuel_used_since_start.value.expect("used") - 20.0).abs() < EPS);
    assert!((e.fuel_used_last_lap.value.expect("last") - 4.0).abs() < EPS);
}

#[test]
fn reserve_is_explicit_in_all_units() {
    let ctx = FuelContext {
        race_finish: RaceFinish::Laps {
            laps_remaining: 15.0,
        },
        lap_progress: Some(0.0),
        ..FuelContext::default()
    };
    let mut config = FuelConfig {
        reserve: ReserveMode::Litres(4.0),
        ..FuelConfig::default()
    };
    let mut r = Run::new(config, 60.0);
    r.laps(5, 4.0);
    let e = r.eng.estimate(&ctx);
    assert!((e.fuel_to_finish.value.expect("ftf") - 60.0).abs() < EPS);
    assert_eq!(e.reserve.litres, Some(4.0));
    assert!((e.reserve.laps.expect("laps") - 1.0).abs() < EPS);
    assert!((e.reserve.percent_of_required.expect("pct") - 100.0 * 4.0 / 60.0).abs() < 1e-9);
    assert!((e.target_fuel.value.expect("target") - 64.0).abs() < EPS);
    // 40 L in the tank: 24 L to add.
    assert!((e.fuel_to_add.value.expect("add") - 24.0).abs() < 1e-9);
    assert!((e.projected_finish_fuel.value.expect("proj") + 20.0).abs() < 1e-9);
    assert!((e.margin_laps.value.expect("margin") + 5.0).abs() < 1e-9);

    config.reserve = ReserveMode::Laps(1.5);
    r.eng.set_config(config);
    assert!((r.eng.estimate(&ctx).reserve.litres.expect("l") - 6.0).abs() < EPS);
    config.reserve = ReserveMode::PercentOfRequired(10.0);
    r.eng.set_config(config);
    assert!((r.eng.estimate(&ctx).reserve.litres.expect("l") - 6.0).abs() < EPS);

    // Default reserve: shown as 0 L, not hidden.
    let d = FuelConfig::default();
    assert_eq!(d.reserve, ReserveMode::Litres(0.0));
}

#[test]
fn incomplete_laps_are_excluded_unless_configured() {
    let mut r = Run::new(FuelConfig::default(), 60.0);
    r.laps(2, 4.0);
    let lap = r.lap(2.0, |i| i.complete = false);
    assert_eq!(lap.exclusion, Some(ExclusionReason::Incomplete));
    let e = r.eng.estimate(&FuelContext::default());
    assert!((e.fuel_per_lap.value.expect("fpl") - 4.0).abs() < EPS);
    assert_eq!(
        e.trace.excluded[0].exclusion,
        Some(ExclusionReason::Incomplete)
    );
    assert_eq!(e.confidence, Some(Confidence::Low), "two laps only");

    let mut config = FuelConfig::default();
    config.exclusions.incomplete = false;
    r.eng.set_config(config);
    let e = r.eng.estimate(&FuelContext::default());
    assert!((e.fuel_per_lap.value.expect("fpl") - 10.0 / 3.0).abs() < 1e-9);
}

#[test]
fn zero_consumption_gives_no_estimate_not_infinity() {
    let mut r = Run::new(FuelConfig::default(), 60.0);
    let lap = r.lap(0.0, |_| {});
    assert_eq!(lap.exclusion, Some(ExclusionReason::NonPositiveConsumption));
    let e = r.eng.estimate(&FuelContext::default());
    assert_eq!(e.fuel_per_lap.value, None);
    assert_eq!(e.laps_remaining.value, None);
    assert_eq!(e.sample_count, 0);
    assert_eq!(e.confidence, None);
    assert!(e.current_fuel.value.is_some());
}

#[test]
fn invalid_consumption_is_rejected() {
    let mut r = Run::new(FuelConfig::default(), 90.0);
    // Non-finite and negative samples are ignored, never read as 0.
    r.sample(Some(f64::NAN));
    r.sample(Some(-3.0));
    r.sample(None);
    let lap = r.lap(60.0, |_| {});
    assert_eq!(lap.exclusion, Some(ExclusionReason::ImplausibleConsumption));
    let e = r.eng.estimate(&FuelContext::default());
    assert_eq!(e.fuel_per_lap.value, None);
    assert!((e.current_fuel.value.expect("fuel") - 30.0).abs() < EPS);
}

#[test]
fn pit_laps_are_excluded_and_configurable() {
    let mut r = Run::new(FuelConfig::default(), 80.0);
    r.laps(3, 4.0);
    assert_eq!(
        r.lap(5.0, |i| i.pit_in = true).exclusion,
        Some(ExclusionReason::PitIn)
    );
    assert_eq!(
        r.lap(4.5, |i| i.pit_out = true).exclusion,
        Some(ExclusionReason::PitOut)
    );
    assert_eq!(
        r.lap(2.0, |i| i.caution = true).exclusion,
        Some(ExclusionReason::Caution)
    );
    assert_eq!(
        r.lap(2.5, |i| i.formation = true).exclusion,
        Some(ExclusionReason::Formation)
    );
    assert_eq!(
        r.lap(3.9, |i| i.invalidated = true).exclusion,
        Some(ExclusionReason::Invalidated)
    );
    let e = r.eng.estimate(&FuelContext::default());
    assert!((e.fuel_per_lap.value.expect("fpl") - 4.0).abs() < EPS);
    assert_eq!(e.trace.included.len(), 3);
    // Five excluded laps plus the partial first lap without fuel data.
    assert_eq!(e.trace.excluded.len(), 6);

    let mut config = FuelConfig::default();
    config.exclusions.pit_in = false;
    r.eng.set_config(config);
    let e = r.eng.estimate(&FuelContext::default());
    assert_eq!(e.trace.included.len(), 4);
    assert!((e.fuel_per_lap.value.expect("fpl") - 17.0 / 4.0).abs() < 1e-9);
}

#[test]
fn confidence_thresholds() {
    let mut r = Run::new(FuelConfig::default(), 100.0);
    r.laps(1, 4.0);
    assert_eq!(
        r.eng.estimate(&FuelContext::default()).confidence,
        Some(Confidence::Low)
    );
    r.laps(2, 4.0);
    assert_eq!(
        r.eng.estimate(&FuelContext::default()).confidence,
        Some(Confidence::Medium)
    );
    r.laps(2, 4.0);
    assert_eq!(
        r.eng.estimate(&FuelContext::default()).confidence,
        Some(Confidence::High)
    );

    // Inconsistent laps: cv above 10 %.
    let mut r = Run::new(FuelConfig::default(), 100.0);
    for used in [3.0, 5.0, 3.0, 5.0, 3.0] {
        r.lap(used, |_| {});
    }
    let e = r.eng.estimate(&FuelContext::default());
    assert_eq!(e.confidence, Some(Confidence::Low));
    assert!(e.std_dev.expect("sd") > 0.9);
}

#[test]
fn windows_select_recent_laps_and_stint() {
    let mut r = Run::new(FuelConfig::default(), 100.0);
    r.laps(5, 3.0);
    r.laps(5, 4.0);
    let w = |r: &Run, w| r.eng.window_trace(w).result_l_per_lap.expect("fpl");
    assert!((w(&r, FuelWindow::Laps1) - 4.0).abs() < EPS);
    assert!((w(&r, FuelWindow::Laps5) - 4.0).abs() < EPS);
    assert!((w(&r, FuelWindow::Laps10) - 3.5).abs() < EPS);
    assert!((w(&r, FuelWindow::Stint) - 3.5).abs() < EPS);
    assert_eq!(FuelWindow::default(), FuelWindow::Laps5);

    // Gradual refuel spread over many samples: one refuel, a new fuel stint, the lap is excluded.
    for _ in 0..40 {
        r.fuel += 1.0;
        let f = r.fuel;
        r.sample(Some(f));
    }
    assert_eq!(r.eng.fuel_stint(), 1);
    let lap = r.lap(4.2, |i| i.pit_out = true);
    assert_eq!(lap.exclusion, Some(ExclusionReason::Refuelled));
    let mut config = FuelConfig::default();
    config.exclusions.pit_out = false;
    r.eng.set_config(config);
    assert_eq!(
        r.eng.laps().last().expect("lap").exclusion,
        Some(ExclusionReason::Refuelled),
        "a refuel exclusion cannot be configured away"
    );
    r.laps(2, 5.0);
    assert!((w(&r, FuelWindow::Stint) - 5.0).abs() < EPS);
    let e = r.eng.estimate(&FuelContext::default());
    // 35 L used before the refuel, then 4.2 + 10 L.
    assert!((e.fuel_used_since_start.value.expect("total") - 49.2).abs() < 1e-9);
    assert!((e.fuel_used_this_stint.value.expect("stint") - 14.2).abs() < 1e-9);
}

#[test]
fn laps_to_finish_for_lap_and_time_races() {
    let lap_race = RaceFinish::lap_race(20, 10, 0.25);
    assert_eq!(lap_race.laps_to_finish(None, None), Some(9.75));
    let timed = RaceFinish::Time {
        time_remaining_s: 250.0,
    };
    assert_eq!(timed.laps_to_finish(Some(0.5), Some(100.0)), Some(3.5));
    // Exactly on a multiple: one more lap (conservative).
    let timed = RaceFinish::Time {
        time_remaining_s: 200.0,
    };
    assert_eq!(timed.laps_to_finish(Some(0.0), Some(100.0)), Some(3.0));
    // Unknown progress or pace: no answer.
    assert_eq!(timed.laps_to_finish(None, Some(100.0)), None);
    assert_eq!(timed.laps_to_finish(Some(0.2), None), None);
    assert_eq!(
        RaceFinish::Unknown.laps_to_finish(Some(0.2), Some(100.0)),
        None
    );
    assert_eq!(
        RaceFinish::lap_race(5, 7, 0.0).laps_to_finish(None, None),
        Some(0.0)
    );
}

#[test]
fn time_race_fuel_to_finish_and_projections() {
    let mut r = Run::new(FuelConfig::default(), 50.0);
    r.laps(5, 4.0);
    let ctx = FuelContext {
        race_finish: RaceFinish::Time {
            time_remaining_s: 250.0,
        },
        lap_progress: Some(0.5),
        laps_until_pit: None,
        laps_until_stint_end: Some(5.0),
    };
    let e = r.eng.estimate(&ctx);
    assert!((e.laps_to_finish.value.expect("laps") - 3.5).abs() < EPS);
    assert!((e.fuel_to_finish.value.expect("ftf") - 14.0).abs() < 1e-9);
    // 30 L now, half a lap to the default pit entry (end of this lap).
    assert!((e.fuel_at_pit_entry.value.expect("pit") - 28.0).abs() < 1e-9);
    assert!((e.fuel_at_stint_end.value.expect("stint end") - 10.0).abs() < 1e-9);
    assert_eq!(e.fuel_to_add.value, Some(0.0));
    assert_eq!(e.refuel_exceeds_tank, Some(false));
}

#[test]
fn no_fuel_data_gives_no_values() {
    let eng = FuelEngine::new(FuelConfig::default());
    let e = eng.estimate(&FuelContext::default());
    assert_eq!(e.current_fuel.value, None);
    assert_eq!(e.fuel_used_since_start.value, None);
    assert_eq!(e.fuel_used_this_lap.value, None);
    assert_eq!(e.fuel_per_lap.value, None);
    assert_eq!(e.laps_remaining.value, None);
    assert_eq!(e.reserve.litres, Some(0.0));
}

#[test]
fn fuel_dropout_makes_current_dependent_outputs_unavailable_not_stale() {
    let ctx = FuelContext {
        race_finish: RaceFinish::Laps {
            laps_remaining: 15.0,
        },
        lap_progress: Some(0.0),
        ..FuelContext::default()
    };
    let mut r = Run::new(FuelConfig::default(), 60.0);
    r.laps(5, 4.0);
    let e = r.eng.estimate(&ctx);
    assert!((e.laps_remaining.value.expect("laps") - 10.0).abs() < EPS);
    // The level channel drops out (garage / spectator view / rejected value).
    r.sample(None);
    let e = r.eng.estimate(&ctx);
    assert_eq!(e.current_fuel.value, None);
    assert_eq!(e.laps_remaining.value, None);
    assert_eq!(e.time_remaining.value, None);
    assert_eq!(e.fuel_to_add.value, None);
    assert_eq!(e.projected_finish_fuel.value, None);
    assert_eq!(e.margin_laps.value, None);
    assert_eq!(e.fuel_at_pit_entry.value, None);
    assert_eq!(e.fuel_used_this_lap.value, None);
    // History-only outputs survive the dropout.
    assert!((e.fuel_per_lap.value.expect("fpl") - 4.0).abs() < EPS);
    // A lap completing during the dropout has unknown consumption, never an older value.
    let lap = r.eng.lap_completed(info(99));
    assert_eq!(lap.used_l, None);
    assert_eq!(lap.exclusion, Some(ExclusionReason::NoFuelData));
}

#[test]
fn short_dropout_mid_lap_keeps_the_lap_measurement() {
    let mut r = Run::new(FuelConfig::default(), 60.0);
    r.laps(2, 4.0);
    r.sample(None);
    let lap = r.lap(4.0, |_| {});
    assert!((lap.used_l.expect("used") - 4.0).abs() < 1e-9);
    assert_eq!(lap.exclusion, None);
}

#[test]
fn refuel_spanning_a_lap_boundary_taints_both_laps() {
    let mut r = Run::new(FuelConfig::default(), 40.0);
    r.laps(5, 4.0); // 20 L left
    // Refuel starts before the line: 20 -> 35 L, then the lap completes mid-fill.
    for f in [25.0, 30.0, 35.0] {
        r.sample(Some(f));
    }
    let a = r.eng.lap_completed(info(50));
    assert_eq!(a.exclusion, Some(ExclusionReason::Refuelled));
    // The fill continues on the new lap to 60 L, then the car uses 4 L.
    for f in [45.0, 55.0, 60.0] {
        r.sample(Some(f));
    }
    r.fuel = 60.0;
    let b = r.lap(4.0, |_| {});
    assert_eq!(b.exclusion, Some(ExclusionReason::Refuelled));
    assert_eq!(b.used_l, None);
    // The following clean lap is measured normally.
    let c = r.lap(4.0, |_| {});
    assert_eq!(c.exclusion, None);
    assert!((c.used_l.expect("used") - 4.0).abs() < 1e-9);
}

/// Gate 1 pitstop1 return to the garage (real values): the level falls from 99.04 L to 22.26 L in 28 ms
/// (ET 315.8025 -> 315.8304) on the very sample on which the lap counter went up. That is LMU setting the
/// level, not 76.8 L of consumption: the lap is excluded as a fuel reset, a new fuel stint starts at the
/// reset level, and the consumption before the reset is kept in the session total.
#[test]
fn garage_return_fuel_reset_is_not_consumption() {
    let mut eng = FuelEngine::new(FuelConfig::default());
    let sample = |et: f64, fuel: f64| FuelSample {
        et_s: et,
        fuel_l: Some(fuel),
        capacity_l: Some(110.0),
    };
    // Full lap 0: 105.0 -> 102.0 L, then lap 1 starts at 102.0 L.
    eng.update(&sample(200.0, 105.0));
    eng.lap_completed(info(0));
    eng.update(&sample(300.0, 102.0));
    let lap0 = eng.lap_completed(info(1));
    assert_eq!(lap0.exclusion, None);
    assert!((lap0.used_l.unwrap_or(f64::NAN) - 3.0).abs() < EPS);
    let stint_before = eng.fuel_stint();
    // Lap 1 is driven down to 99.04 L, then the garage reset on the counter-increment sample.
    eng.update(&sample(315.8025, 99.04));
    eng.update(&sample(315.8304, 22.26));
    let lap1 = eng.lap_completed(info(2));
    assert_eq!(lap1.exclusion, Some(ExclusionReason::FuelReset));
    assert_eq!(lap1.used_l, None);
    assert_eq!(eng.fuel_stint(), stint_before + 1);
    let e = eng.estimate(&FuelContext::default());
    // 105.0 -> 99.04 L were burnt before the reset; nothing since.
    assert!((e.fuel_used_since_start.value.unwrap_or(f64::NAN) - 5.96).abs() < 1e-6);
    assert_eq!(e.fuel_used_this_stint.value, Some(0.0));
    // The next lap is measured from the reset level again.
    eng.update(&sample(415.8, 19.26));
    let lap2 = eng.lap_completed(info(3));
    assert_eq!(lap2.exclusion, None);
    assert!((lap2.used_l.unwrap_or(f64::NAN) - 3.0).abs() < 1e-6);
}

/// Consumption is never a reset: a large drop over a long time (a data gap while driving) and a drop at
/// the fastest real burn rate are consumption; only a drop faster than [`MAX_FUEL_BURN_L_PER_S`] (and
/// larger than [`FUEL_RESET_MIN_DROP_L`]) is a reset.
#[test]
fn fuel_reset_needs_a_drop_faster_than_any_burn() {
    let mut eng = FuelEngine::new(FuelConfig::default());
    let sample = |et: f64, fuel: f64| FuelSample {
        et_s: et,
        fuel_l: Some(fuel),
        capacity_l: Some(110.0),
    };
    eng.update(&sample(0.0, 80.0));
    eng.lap_completed(info(0));
    // 10 L over a 60 s gap (0.17 L/s) and 0.4 L in 0.01 s (below the minimum drop): consumption.
    eng.update(&sample(60.0, 70.0));
    eng.update(&sample(60.01, 69.6));
    let lap = eng.lap_completed(info(1));
    assert_eq!(lap.exclusion, None);
    assert!((lap.used_l.unwrap_or(f64::NAN) - 10.4).abs() < 1e-9);
    // A missing value in between does not hide a reset: the rate is judged from the last valid sample.
    eng.update(&FuelSample {
        et_s: 60.02,
        fuel_l: None,
        capacity_l: Some(110.0),
    });
    eng.update(&sample(60.03, 30.0));
    let lap = eng.lap_completed(info(2));
    assert_eq!(lap.exclusion, Some(ExclusionReason::FuelReset));
    // A refuel afterwards is still a refuel.
    eng.update(&sample(70.0, 90.0));
    let lap = eng.lap_completed(info(3));
    assert_eq!(lap.exclusion, Some(ExclusionReason::Refuelled));
}

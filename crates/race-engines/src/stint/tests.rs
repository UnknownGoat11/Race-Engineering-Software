//! Stint engine tests.

use super::*;

fn s(
    et: f64,
    phase: PitPhase,
    fuel: Option<f64>,
    wear: [Option<f64>; WHEELS],
) -> StintSample<'static> {
    StintSample {
        et_s: et,
        pit_phase: phase,
        fuel_l: fuel,
        tyre_wear: wear,
        front_compound: Some("Medium"),
        rear_compound: Some("Medium"),
    }
}

fn lap(t: f64, fuel: Option<f64>) -> StintLap {
    StintLap {
        lap_time_s: Some(t),
        pace_eligible: true,
        fuel_used_l: fuel,
        energy_used_frac: None,
    }
}

const W: [Option<f64>; WHEELS] = [Some(0.9); WHEELS];

#[test]
fn stop_starts_a_new_stint_and_drive_through_does_not() {
    let mut e = StintEngine::new();
    let ev = e.update(&s(100.0, PitPhase::OnTrack, Some(80.0), W));
    assert_eq!(
        ev,
        vec![StintEvent::Started {
            number: 1,
            reason: StintStart::SessionStart,
            et_s: 100.0,
            tyres_changed: [false; WHEELS]
        }]
    );
    // Drive-through: lane without a stop.
    let mut ev = Vec::new();
    for (et, p) in [
        (200.0, PitPhase::Entering),
        (220.0, PitPhase::Exiting),
        (230.0, PitPhase::OnTrack),
    ] {
        ev.extend(e.update(&s(et, p, Some(70.0), W)));
    }
    assert!(ev.is_empty(), "{ev:?}");
    assert_eq!(e.snapshot().number.value, Some(1));

    // Stop: stint 1 ends at the lane entry, stint 2 starts at the exit.
    let mut ev = Vec::new();
    for (et, p) in [
        (300.0, PitPhase::Entering),
        (310.0, PitPhase::Stopped),
        (340.0, PitPhase::Exiting),
        (350.0, PitPhase::OnTrack),
    ] {
        ev.extend(e.update(&s(et, p, Some(60.0), W)));
    }
    assert_eq!(ev.len(), 2, "{ev:?}");
    assert_eq!(
        ev[0],
        StintEvent::Ended {
            number: 1,
            et_s: 300.0
        }
    );
    assert!(matches!(
        ev[1],
        StintEvent::Started {
            number: 2,
            reason: StintStart::PitStop,
            et_s: 350.0,
            ..
        }
    ));
    let table = e.table();
    assert_eq!(table.len(), 2);
    assert_eq!(table[0].time_s, 200.0);
    assert!(!table[0].running);
    assert!(table[1].running);
    let snap = e.snapshot();
    assert_eq!(snap.number.value, Some(2));
    assert_eq!(snap.number.formula, formula::STINT_NUMBER);
    e.update(&s(400.0, PitPhase::OnTrack, Some(58.0), W));
    assert_eq!(e.snapshot().time.value, Some(50.0));
}

#[test]
fn session_starting_in_the_garage_begins_with_a_garage_exit() {
    let mut e = StintEngine::new();
    assert!(e.update(&s(5.0, PitPhase::Garage, None, W)).is_empty());
    assert_eq!(e.snapshot().number.value, None);
    e.update(&s(10.0, PitPhase::Exiting, None, W));
    let ev = e.update(&s(20.0, PitPhase::OnTrack, None, W));
    assert!(matches!(
        ev[..],
        [StintEvent::Started {
            number: 1,
            reason: StintStart::GarageExit,
            ..
        }]
    ));
}

#[test]
fn tyre_change_is_a_wear_step_while_stopped() {
    let mut e = StintEngine::new();
    e.update(&s(0.0, PitPhase::OnTrack, None, W));
    e.lap_completed(&lap(100.0, None));
    let snap = e.snapshot();
    // No change observed: ages are lower bounds, the set age is not published.
    assert_eq!(snap.tyre_age_laps, [1; WHEELS]);
    assert_eq!(snap.tyre_age_lower_bound, [true; WHEELS]);
    assert_eq!(snap.tyre_set_laps.value, None);

    // Fronts changed only (direction-agnostic: the step size counts).
    let changed = [Some(1.0), Some(1.0), Some(0.9), Some(0.9)];
    e.update(&s(100.0, PitPhase::Entering, None, W));
    e.update(&s(110.0, PitPhase::Stopped, None, W));
    e.update(&s(130.0, PitPhase::Stopped, None, changed));
    e.update(&s(135.0, PitPhase::Exiting, None, changed));
    let ev = e.update(&s(140.0, PitPhase::OnTrack, None, changed));
    assert!(matches!(
        ev[1],
        StintEvent::Started {
            tyres_changed: [true, true, false, false],
            ..
        }
    ));
    let snap = e.snapshot();
    assert_eq!(snap.tyre_age_laps, [0, 0, 1, 1]);
    assert_eq!(snap.tyre_set_laps.value, None);

    // Full change: the set age is known from now.
    let fresh = [Some(0.5); WHEELS];
    e.update(&s(200.0, PitPhase::Entering, None, changed));
    e.update(&s(210.0, PitPhase::Stopped, None, changed));
    e.update(&s(230.0, PitPhase::Exiting, None, fresh));
    e.update(&s(240.0, PitPhase::OnTrack, None, fresh));
    assert_eq!(e.snapshot().tyre_set_laps.value, Some(0));
    e.update(&s(300.0, PitPhase::OnTrack, None, [Some(0.49); WHEELS]));
    e.lap_completed(&lap(100.0, None));
    let snap = e.snapshot();
    assert_eq!(snap.tyre_set_laps.value, Some(1));
    let wear = snap.wear_per_lap[0].value.expect("wear per lap");
    assert!((wear - 0.01).abs() < 1e-9, "{wear}");
}

#[test]
fn small_wear_noise_in_the_box_is_not_a_change() {
    let mut e = StintEngine::new();
    e.update(&s(0.0, PitPhase::OnTrack, None, W));
    e.update(&s(10.0, PitPhase::Entering, None, W));
    e.update(&s(20.0, PitPhase::Stopped, None, W));
    let ev_wear = [Some(0.9005); WHEELS];
    e.update(&s(40.0, PitPhase::Exiting, None, ev_wear));
    let ev = e.update(&s(50.0, PitPhase::OnTrack, None, ev_wear));
    assert!(matches!(
        ev[1],
        StintEvent::Started {
            tyres_changed: [false, false, false, false],
            ..
        }
    ));
}

#[test]
fn pace_average_and_trend_over_eligible_laps() {
    let mut e = StintEngine::new();
    e.update(&s(0.0, PitPhase::OnTrack, None, W));
    // Out-lap style ineligible lap first.
    e.lap_completed(&StintLap {
        lap_time_s: Some(130.0),
        pace_eligible: false,
        ..StintLap::default()
    });
    for i in 0..6 {
        e.lap_completed(&lap(100.0 + 0.2 * f64::from(i), Some(3.0)));
    }
    let snap = e.snapshot();
    assert_eq!(snap.laps.value, Some(7));
    // Mean of 100.0..=101.0 step 0.2 = 100.5.
    let avg = snap.average_pace.value.expect("avg");
    assert!((avg - 100.5).abs() < 1e-9);
    // Trend over the last 5 eligible laps: +0.2 s/lap.
    let trend = snap.lap_time_trend.value.expect("trend");
    assert!((trend - 0.2).abs() < 1e-9, "{trend}");
    assert_eq!(snap.trend_laps, TREND_WINDOW_LAPS);
    assert_eq!(
        snap.lap_time_trend.provenance,
        telemetry_model::Provenance::Estimated
    );
    assert_eq!(snap.fuel_per_lap.value, Some(3.0));
}

#[test]
fn trend_needs_three_laps() {
    let mut e = StintEngine::new();
    e.update(&s(0.0, PitPhase::OnTrack, None, W));
    e.lap_completed(&lap(100.0, None));
    e.lap_completed(&lap(101.0, None));
    assert_eq!(e.snapshot().lap_time_trend.value, None);
}

#[test]
fn fuel_used_ignores_the_refuel_and_noise() {
    let mut e = StintEngine::new();
    e.update(&s(0.0, PitPhase::OnTrack, Some(100.0), W));
    e.update(&s(10.0, PitPhase::OnTrack, Some(95.0), W));
    // Slosh noise upwards below the refuel threshold.
    e.update(&s(11.0, PitPhase::OnTrack, Some(95.3), W));
    e.update(&s(20.0, PitPhase::OnTrack, Some(90.0), W));
    assert_eq!(e.snapshot().fuel_used.value, Some(10.0));
    e.update(&s(30.0, PitPhase::Entering, Some(89.5), W));
    e.update(&s(40.0, PitPhase::Stopped, Some(89.5), W));
    e.update(&s(50.0, PitPhase::Stopped, Some(110.0), W));
    e.update(&s(60.0, PitPhase::Exiting, Some(110.0), W));
    e.update(&s(70.0, PitPhase::OnTrack, Some(109.8), W));
    let t = e.table();
    assert_eq!(t[0].fuel_used_l, Some(10.5));
    let s2 = e.current().expect("stint 2");
    assert_eq!(s2.fuel_at_start_l, Some(109.8));
    assert_eq!(e.snapshot().fuel_used.value, Some(0.0));
}

#[test]
fn unavailable_inputs_stay_unavailable() {
    let mut e = StintEngine::new();
    e.update(&s(0.0, PitPhase::OnTrack, None, [None; WHEELS]));
    e.lap_completed(&StintLap::default());
    let snap = e.snapshot();
    assert_eq!(snap.fuel_used.value, None);
    assert_eq!(snap.fuel_per_lap.value, None);
    assert_eq!(snap.energy_per_lap.value, None);
    assert_eq!(snap.average_pace.value, None);
    assert!(snap.wear_per_lap.iter().all(|m| m.value.is_none()));
    assert_eq!(snap.laps.value, Some(1));
}

#[test]
fn stint_time_survives_a_session_end_rewind() {
    let mut e = StintEngine::new();
    e.update(&s(1_000.0, PitPhase::OnTrack, None, W));
    e.update(&s(1_100.0, PitPhase::OnTrack, None, W));
    // LMU rewinds ET after the session end.
    e.update(&s(1_057.0, PitPhase::OnTrack, None, W));
    assert_eq!(e.snapshot().time.value, Some(100.0));
    e.update(&s(1_060.0, PitPhase::OnTrack, None, W));
    assert_eq!(e.snapshot().time.value, Some(103.0));
}

#[test]
fn energy_and_max_laps() {
    let mut e = StintEngine::new();
    e.update(&s(0.0, PitPhase::OnTrack, None, W));
    e.lap_completed(&StintLap {
        energy_used_frac: Some(0.08),
        ..lap(100.0, Some(4.0))
    });
    assert_eq!(e.snapshot().energy_per_lap.value, Some(0.08));
    // 100 L at 4 L/lap = 25 laps; energy 1 / 0.08 = 12.5 laps: the limit is energy.
    assert_eq!(
        stint_max_laps(Some(100.0), Some(4.0), Some(0.08), Some(true)).value,
        Some(12.5)
    );
    assert_eq!(
        stint_max_laps(Some(100.0), Some(4.0), None, Some(false)).value,
        Some(25.0)
    );
    assert_eq!(
        stint_max_laps(None, Some(4.0), None, Some(false)).value,
        None
    );
    assert_eq!(
        stint_max_laps(Some(100.0), Some(0.0), None, Some(false)).value,
        None
    );
    // Regression: an energy-limited car with VE per lap not known yet must not show the fuel-only limit.
    assert_eq!(
        stint_max_laps(Some(100.0), Some(4.0), None, Some(true)).value,
        None
    );
    // Unknown whether the rule applies: no limit.
    assert_eq!(
        stint_max_laps(Some(100.0), Some(4.0), Some(0.08), None).value,
        None
    );
}

/// Regression: after a refuel was detected, a further gradual rise below the refuel threshold was not taken
/// as the new segment start, understating the stint fuel by up to that rise.
#[test]
fn gradual_refuel_inside_a_stint_counts_the_full_fill() {
    let mut e = StintEngine::new();
    e.update(&s(0.0, PitPhase::OnTrack, Some(100.0), W));
    e.update(&s(10.0, PitPhase::OnTrack, Some(50.0), W));
    // Refuel detected at 60.0 L, then the level settles at 60.4 L in small steps.
    for (et, f) in [(11.0, 60.0), (12.0, 60.2), (13.0, 60.4)] {
        e.update(&s(et, PitPhase::OnTrack, Some(f), W));
    }
    e.update(&s(20.0, PitPhase::OnTrack, Some(50.0), W));
    let used = e.snapshot().fuel_used.value.expect("used");
    assert!((used - 60.4).abs() < 1e-9, "{used}");
}

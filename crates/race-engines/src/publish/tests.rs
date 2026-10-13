//! Publication tests.

use telemetry_model::{Quality, registry};

use super::*;
use crate::fuel::{FuelConfig, FuelContext, FuelEngine, FuelLapInfo, FuelSample, RaceFinish};
use crate::race::{PitPhase, RaceEngine, RaceSample};
use crate::stint::{StintEngine, StintLap, StintSample, stint_max_laps};
use crate::timing::{LapId, TimingConfig, TimingEngine};

fn frame() -> TelemetryFrame {
    TelemetryFrame::new(1, 0, Some(0.0))
}

fn quality(f: &TelemetryFrame, id: ChannelId) -> Quality {
    f.get(id).quality
}

/// A fuel engine with four 3 L laps and 40 L left.
fn fuel_estimate() -> FuelEstimate {
    let mut e = FuelEngine::new(FuelConfig::default());
    let mut fuel = 52.0;
    let mut et = 0.0;
    e.update(&FuelSample {
        et_s: et,
        fuel_l: Some(fuel),
        capacity_l: Some(100.0),
    });
    for lap in 0..5_u32 {
        for _ in 0..10 {
            if lap > 0 {
                fuel -= 0.3;
            }
            et += 10.0;
            e.update(&FuelSample {
                et_s: et,
                fuel_l: Some(fuel),
                capacity_l: Some(100.0),
            });
        }
        e.lap_completed(FuelLapInfo {
            lap_id: LapId(lap),
            lap_number: i32::try_from(lap).ok(),
            lap_time_s: Some(100.0),
            complete: lap > 0,
            pit_in: false,
            pit_out: false,
            formation: false,
            caution: false,
            invalidated: false,
        });
    }
    e.estimate(&FuelContext {
        race_finish: RaceFinish::Laps {
            laps_remaining: 10.0,
        },
        lap_progress: Some(0.0),
        ..FuelContext::default()
    })
}

fn stint_snapshot() -> crate::stint::StintSnapshot {
    let mut s = StintEngine::new();
    s.update(&StintSample {
        et_s: 10.0,
        pit_phase: PitPhase::OnTrack,
        ..StintSample::default()
    });
    s.update(&StintSample {
        et_s: 110.0,
        pit_phase: PitPhase::OnTrack,
        ..StintSample::default()
    });
    s.lap_completed(&StintLap {
        lap_time_s: Some(100.0),
        pace_eligible: true,
        ..StintLap::default()
    });
    s.snapshot()
}

#[test]
fn nothing_known_publishes_unavailable_never_zero() {
    let mut f = frame();
    let r = publish(&mut f, &EngineOutputs::default());
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.valid, 1);
    assert_eq!(r.unavailable, OWNED_CHANNELS.len() - 1);
    for &id in OWNED_CHANNELS {
        if id == ch::CALC_ENGINE_VERSION {
            assert_eq!(
                f.get(id).value,
                Value::U32(crate::CALCULATION_ENGINE_VERSION)
            );
        } else {
            assert_eq!(quality(&f, id), Quality::Unavailable, "0x{:04X}", id.0);
            assert_eq!(f.get(id).value, Value::None);
        }
    }
}

#[test]
fn engine_values_are_written_with_registry_types() {
    let fuel = fuel_estimate();
    assert_eq!(
        fuel.fuel_per_lap.value.map(|v| (v * 1e9).round() / 1e9),
        Some(3.0)
    );
    let stint = stint_snapshot();
    let mut race = RaceEngine::new();
    race.update(&Default::default(), &RaceSample::default());
    let timing = TimingEngine::new(TimingConfig::default())
        .expect("timing")
        .snapshot();
    let max = stint_max_laps(Some(100.0), fuel.fuel_per_lap.value, None);
    let mut f = frame();
    let r = publish(
        &mut f,
        &EngineOutputs {
            timing: Some(&timing),
            fuel: Some(&fuel),
            stint: Some(&stint),
            stint_max_laps: Some(max),
            race: race.snapshot(),
        },
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(r.out_of_range.is_empty(), "{:?}", r.out_of_range);
    match f.get(ch::FUEL_PER_LAP).value {
        Value::F32(v) => assert!((f64::from(v) - 3.0).abs() < 1e-5),
        ref other => panic!("{other:?}"),
    }
    assert_eq!(f.get(ch::STINT_NUMBER).value, Value::U32(1));
    assert_eq!(f.get(ch::STINT_LAPS).value, Value::U32(1));
    assert_eq!(f.get(ch::STINT_TIME).value, Value::F64(100.0));
    match f.get(ch::STINT_MAX_LAPS).value {
        Value::F32(v) => assert!((f64::from(v) - 100.0 / 3.0).abs() < 1e-3),
        ref other => panic!("{other:?}"),
    }
    // No tyre change observed: the set age is unknown, not 1.
    assert_eq!(quality(&f, ch::TYRE_SET_LAPS), Quality::Unavailable);
    // Without a reference lap the delta is unknown.
    assert_eq!(quality(&f, ch::DELTA_PERSONAL_BEST), Quality::Unavailable);
    assert_eq!(quality(&f, ch::RACE_LAPS_REMAINING), Quality::Unavailable);

    // The next step without outputs clears every value: nothing lingers.
    let r = publish(&mut f, &EngineOutputs::default());
    assert!(r.errors.is_empty());
    assert_eq!(quality(&f, ch::FUEL_PER_LAP), Quality::Unavailable);
    assert_eq!(quality(&f, ch::STINT_NUMBER), Quality::Unavailable);
}

#[test]
fn out_of_range_values_are_invalid_not_clamped() {
    let mut f = frame();
    let r = publish(
        &mut f,
        &EngineOutputs {
            stint_max_laps: Some(stint_max_laps(Some(100.0), Some(1e-9), None)),
            ..EngineOutputs::default()
        },
    );
    assert_eq!(r.out_of_range, vec![ch::STINT_MAX_LAPS]);
    assert_eq!(quality(&f, ch::STINT_MAX_LAPS), Quality::Invalid);
    assert_eq!(f.get(ch::STINT_MAX_LAPS).value, Value::None);
}

#[test]
fn a_formula_that_differs_from_the_registry_is_reported() {
    let mut f = frame();
    let wrong = Metric::estimated("not_the_registry_formula", Some(10.0));
    let r = publish(
        &mut f,
        &EngineOutputs {
            stint_max_laps: Some(wrong),
            ..EngineOutputs::default()
        },
    );
    assert!(matches!(
        r.errors[..],
        [PublishError::ContractMismatch { channel, .. }] if channel == ch::STINT_MAX_LAPS.0
    ));
}

/// Every owned calculated channel: the engine metric's formula is the registry origin and the provenance
/// matches (checked on the metrics the engines really produce, with or without a value).
#[test]
fn engine_metrics_match_the_registry_contract() {
    let fuel = FuelEngine::new(FuelConfig::default()).estimate(&FuelContext::default());
    let stint = StintEngine::new().snapshot();
    let mut race = RaceEngine::new();
    race.update(&Default::default(), &RaceSample::default());
    let race = race.snapshot().expect("race").clone();
    let timing = TimingEngine::new(TimingConfig::default())
        .expect("timing")
        .snapshot();
    let pairs: Vec<(ChannelId, &'static str, Provenance)> = vec![
        (
            ch::DELTA_PERSONAL_BEST,
            timing.delta_to_personal_best.formula,
            timing.delta_to_personal_best.provenance,
        ),
        (
            ch::PREDICTED_LAP_TIME,
            timing.predicted_lap.formula,
            timing.predicted_lap.provenance,
        ),
        (
            ch::FUEL_USED_LAST_LAP,
            fuel.fuel_used_last_lap.formula,
            fuel.fuel_used_last_lap.provenance,
        ),
        (
            ch::FUEL_PER_LAP,
            fuel.fuel_per_lap.formula,
            fuel.fuel_per_lap.provenance,
        ),
        (
            ch::FUEL_USED_THIS_LAP,
            fuel.fuel_used_this_lap.formula,
            fuel.fuel_used_this_lap.provenance,
        ),
        (
            ch::FUEL_LAPS_REMAINING,
            fuel.laps_remaining.formula,
            fuel.laps_remaining.provenance,
        ),
        (
            ch::FUEL_TIME_REMAINING,
            fuel.time_remaining.formula,
            fuel.time_remaining.provenance,
        ),
        (
            ch::FUEL_TO_FINISH,
            fuel.fuel_to_finish.formula,
            fuel.fuel_to_finish.provenance,
        ),
        (
            ch::FUEL_TO_ADD,
            fuel.fuel_to_add.formula,
            fuel.fuel_to_add.provenance,
        ),
        (
            ch::FUEL_MARGIN_LAPS,
            fuel.margin_laps.formula,
            fuel.margin_laps.provenance,
        ),
        (
            ch::STINT_NUMBER,
            stint.number.formula,
            stint.number.provenance,
        ),
        (ch::STINT_LAPS, stint.laps.formula, stint.laps.provenance),
        (ch::STINT_TIME, stint.time.formula, stint.time.provenance),
        (
            ch::TYRE_SET_LAPS,
            stint.tyre_set_laps.formula,
            stint.tyre_set_laps.provenance,
        ),
        (
            ch::STINT_MAX_LAPS,
            stint_max_laps(None, None, None).formula,
            stint_max_laps(None, None, None).provenance,
        ),
        (
            ch::RACE_LAPS_REMAINING,
            race.race_laps_remaining.formula,
            race.race_laps_remaining.provenance,
        ),
    ];
    assert_eq!(pairs.len(), OWNED_CHANNELS.len() - 1);
    for (id, formula, provenance) in pairs {
        assert!(OWNED_CHANNELS.contains(&id));
        let def = registry::by_id(id).expect("registered");
        assert_eq!(def.origin, format!("formula:{formula}"), "{}", def.name);
        assert_eq!(def.provenance, provenance, "{}", def.name);
    }
}

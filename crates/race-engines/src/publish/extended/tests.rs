//! Extended publication tests.

use telemetry_model::enums::{GamePhase, SessionType, YellowFlagState};
use telemetry_model::scoring::{CompetitorEntry, ScoringState, SessionScoring};
use telemetry_model::{Quality, registry};

use super::*;
use crate::aids::{AidsEngine, AidsSample};
use crate::brake::{BrakeConfig, BrakeEngine, BrakeSample, BrakeThresholds};
use crate::race::{RaceEngine, RaceSample};
use crate::tyre::{TyreConfig, TyreEngine, TyreSample, TyreThresholds, TyreWheelSample};

fn frame() -> TelemetryFrame {
    TelemetryFrame::new(1, 0, Some(0.0))
}

fn quality(f: &TelemetryFrame, id: ChannelId) -> Quality {
    f.get(id).quality
}

fn f32_of(f: &TelemetryFrame, id: ChannelId) -> f64 {
    match f.get(id).value {
        Value::F32(v) => f64::from(v),
        ref other => panic!("0x{:04X}: {other:?}", id.0),
    }
}

/// A tyre engine with surface 80/85/90 °C (left/centre/right), layer 95, carcass 100, pressure 170 kPa on
/// every wheel, warming by 1 °C and 1 kPa per second for 5 s, with thresholds that put the surface in range.
fn tyre_snapshot() -> TyreSnapshot {
    let cfg = TyreConfig {
        thresholds: TyreThresholds {
            surface_low_c: Some(70.0),
            surface_high_c: Some(100.0),
            pressure_low_pa: Some(160_000.0),
            pressure_high_pa: Some(170_500.0),
            edge_spread_max_c: Some(5.0),
        },
        ..TyreConfig::default()
    };
    let mut e = TyreEngine::new(cfg).expect("config");
    for i in 0..=5 {
        let d = f64::from(i);
        let w = TyreWheelSample {
            pressure_pa: Some(170_000.0 + 1_000.0 * d),
            surface_left_c: Some(80.0 + d),
            surface_center_c: Some(85.0 + d),
            surface_right_c: Some(90.0 + d),
            layer_left_c: Some(95.0),
            layer_center_c: Some(95.0),
            layer_right_c: Some(95.0),
            carcass_c: Some(100.0 + d),
        };
        e.update(&TyreSample {
            et_s: 10.0 + d,
            wheels: [w; WHEELS],
        });
    }
    e.snapshot()
}

fn brake_snapshot() -> BrakeSnapshot {
    let cfg = BrakeConfig {
        thresholds: BrakeThresholds {
            temp_low_c: Some(200.0),
            temp_high_c: Some(800.0),
        },
        ..BrakeConfig::default()
    };
    let mut e = BrakeEngine::new(cfg).expect("config");
    for (i, front) in [400.0, 500.0, 450.0].into_iter().enumerate() {
        e.update(&BrakeSample {
            et_s: 10.0 + i as f64,
            temps_c: [Some(front), Some(front), Some(300.0), Some(300.0)],
            bias_rear: Some(0.45),
        });
    }
    e.snapshot()
}

fn car(slot: i32, place: u8, class: &str, behind_leader_s: f64, best_s: f64) -> CompetitorEntry {
    CompetitorEntry {
        place: Some(place),
        class_name: Some(class.into()),
        laps: Some(10),
        laps_behind_leader: Some(0),
        laps_behind_next: Some(0),
        time_behind_leader_s: Some(behind_leader_s),
        best_lap_s: Some(best_s),
        ..CompetitorEntry::new(slot)
    }
}

/// Leader (Hypercar), an LMGT3 car, the player (Hypercar, P3) and a Hypercar behind, in `session`.
fn race_snapshot(session: SessionType, phase: GamePhase) -> RaceSnapshot {
    let mut player = car(3, 3, "Hypercar", 5.5, 101.0);
    player.is_player = true;
    let scoring = ScoringState {
        session: SessionScoring {
            session_type: Some(session),
            game_phase: Some(phase),
            yellow_flag_state: Some(YellowFlagState::NoFlag),
            current_et_s: Some(1_200.0),
            time_remaining_s: Some(250.0),
            ..Default::default()
        },
        vehicles: vec![
            car(1, 1, "Hypercar", 0.0, 100.0),
            car(2, 2, "LMGT3", 3.0, 110.0),
            player,
            car(4, 4, "Hypercar", 7.25, 102.5),
        ],
    };
    let mut e = RaceEngine::new();
    e.update(
        &scoring,
        &RaceSample {
            et_s: 1_200.0,
            speed_mps: Some(60.0),
            in_pit_lane: Some(false),
            lap_progress: Some(0.5),
        },
    );
    e.snapshot().expect("snapshot").clone()
}

fn aids_snapshot() -> AidsSnapshot {
    let mut e = AidsEngine::new();
    e.lap_completed();
    for i in 0..4 {
        e.update(&AidsSample {
            et_s: f64::from(i),
            tc_active: Some(i == 0),
            abs_active: Some(false),
        });
    }
    e.lap_completed();
    e.snapshot()
}

#[test]
fn nothing_known_publishes_unavailable_never_zero() {
    let mut f = frame();
    let r = publish_extended(&mut f, &ExtendedOutputs::default());
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.valid, 0);
    assert_eq!(r.unavailable, EXTENDED_CHANNELS.len());
    for &id in EXTENDED_CHANNELS {
        assert_eq!(quality(&f, id), Quality::Unavailable, "0x{:04X}", id.0);
        assert_eq!(f.get(id).value, Value::None);
    }
}

#[test]
fn channel_list_is_unique_and_calculated() {
    let mut seen = std::collections::BTreeSet::new();
    for &id in EXTENDED_CHANNELS {
        assert!(seen.insert(id), "0x{:04X} listed twice", id.0);
        let def = registry::by_id(id).expect("registered");
        assert_eq!(
            def.source,
            telemetry_model::Source::Calculated,
            "{}",
            def.name
        );
    }
}

#[test]
fn engine_values_are_written_with_registry_types_and_contracts() {
    let tyre = tyre_snapshot();
    let brake = brake_snapshot();
    let race = race_snapshot(SessionType::Race(1), GamePhase::GreenFlag);
    let aids = aids_snapshot();
    let mut f = frame();
    let r = publish_extended(
        &mut f,
        &ExtendedOutputs {
            tyre: Some(&tyre),
            brake: Some(&brake),
            map_lap_distance: Some(Metric::derived(formula::TRACK_PROJECTION, Some(1_234.5))),
            race: Some(&race),
            aids: Some(&aids),
        },
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert!(r.out_of_range.is_empty(), "{:?}", r.out_of_range);

    // Tyres (every wheel identical): pressure +5 kPa, surface avg 85 -> 90, carcass +5.
    for w in Wheel::ALL {
        let i = u16::try_from(w.index()).expect("wheel");
        let at = |fl: ChannelId| ChannelId(fl.0 + i);
        assert!((f32_of(&f, at(ch::TYRE_PRESSURE_DELTA_FL)) - 5_000.0).abs() < 1e-3);
        assert!((f32_of(&f, at(ch::TYRE_TEMP_DELTA_FL)) - 5.0).abs() < 1e-4);
        assert!((f32_of(&f, at(ch::TYRE_CARCASS_TEMP_DELTA_FL)) - 5.0).abs() < 1e-4);
        assert!((f32_of(&f, at(ch::TYRE_TEMP_PEAK_FL)) - 90.0).abs() < 1e-4);
        assert!((f32_of(&f, at(ch::TYRE_LAYER_TEMP_AVG_FL)) - 95.0).abs() < 1e-4);
        assert!((f32_of(&f, at(ch::TYRE_TEMP_ROLLING_AVG_FL)) - 87.5).abs() < 1e-4);
        // Surface 90 in range; pressure 175 kPa above 170.5 kPa; |inner − outer| = 10 > 5.
        assert_eq!(
            f.get(at(ch::TYRE_TEMP_BAND_FL)).value,
            Value::U8(BAND_IN_RANGE)
        );
        assert_eq!(
            f.get(at(ch::TYRE_PRESSURE_BAND_FL)).value,
            Value::U8(BAND_HIGH)
        );
        assert_eq!(
            f.get(at(ch::TYRE_EDGE_SPREAD_BAND_FL)).value,
            Value::U8(BAND_HIGH)
        );
    }
    // Inner/outer follow the wheel side (A-LMU-016): FL inner = right edge (95), FR inner = left edge (85).
    assert!((f32_of(&f, ch::TYRE_TEMP_INNER_MIDDLE_FL) - 5.0).abs() < 1e-4);
    assert!((f32_of(&f, ch::TYRE_TEMP_INNER_MIDDLE_FR) + 5.0).abs() < 1e-4);
    assert!((f32_of(&f, ch::TYRE_TEMP_BALANCE_FRONT_REAR)).abs() < 1e-4);
    assert!((f32_of(&f, ch::TYRE_PRESSURE_BALANCE_LEFT_RIGHT)).abs() < 1e-4);

    // Brakes: peak 500 front / 300 rear, front − rear 150, front bias 1 − 0.45.
    assert!((f32_of(&f, ch::BRAKE_TEMP_PEAK_FL) - 500.0).abs() < 1e-3);
    assert!((f32_of(&f, ch::BRAKE_TEMP_PEAK_RR) - 300.0).abs() < 1e-3);
    assert_eq!(
        f.get(ch::BRAKE_TEMP_BAND_FL).value,
        Value::U8(BAND_IN_RANGE)
    );
    assert!((f32_of(&f, ch::BRAKE_TEMP_FRONT_REAR) - 150.0).abs() < 1e-3);
    assert!((f32_of(&f, ch::BRAKE_BIAS_FRONT) - 0.55).abs() < 1e-6);

    assert_eq!(f.get(ch::MAP_LAP_DISTANCE).value, Value::F64(1_234.5));

    // Race classification: class gap to the leader 5.5 s on the same lap, 1.75 s to the car behind.
    assert_eq!(f.get(ch::CLASS_GAP_AHEAD).value, Value::F64(5.5));
    assert_eq!(f.get(ch::CLASS_GAP_AHEAD_LAPS).value, Value::I32(0));
    assert_eq!(f.get(ch::CLASS_GAP_BEHIND).value, Value::F64(1.75));
    // Best-lap gaps belong to practice / qualifying: no data in a race.
    for id in [
        ch::BEST_LAP_GAP_AHEAD,
        ch::BEST_LAP_GAP_BEHIND,
        ch::CLASS_BEST_LAP_GAP_AHEAD,
        ch::CLASS_BEST_LAP_GAP_BEHIND,
    ] {
        assert_eq!(quality(&f, id), Quality::Unavailable, "0x{:04X}", id.0);
    }
    assert_eq!(
        f.get(ch::FLAG_SUMMARY).value,
        Value::U8(flag_summary_code(RaceFlag::Green).expect("code"))
    );
    assert_eq!(f.get(ch::FORMATION_LAP).value, Value::Bool(false));

    assert!((f32_of(&f, ch::TC_DUTY) - 0.25).abs() < 1e-6);
    assert_eq!(f32_of(&f, ch::ABS_DUTY), 0.0);

    // The next step without outputs clears every value: nothing lingers.
    let r = publish_extended(&mut f, &ExtendedOutputs::default());
    assert_eq!(r.unavailable, EXTENDED_CHANNELS.len());
    assert_eq!(quality(&f, ch::TYRE_TEMP_PEAK_FL), Quality::Unavailable);
}

#[test]
fn best_lap_sessions_route_gaps_to_the_best_lap_channels() {
    let race = race_snapshot(SessionType::Qualifying(1), GamePhase::GreenFlag);
    let mut f = frame();
    let r = publish_extended(
        &mut f,
        &ExtendedOutputs {
            race: Some(&race),
            ..ExtendedOutputs::default()
        },
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    // Overall: P2 (LMGT3, 110.0) ahead is slower than the player (101.0): a negative gap is no data; the car
    // behind is 1.5 s slower. Class: the leader 1.0 s faster, the Hypercar behind 1.5 s slower.
    assert_eq!(quality(&f, ch::BEST_LAP_GAP_AHEAD), Quality::Unavailable);
    assert_eq!(f.get(ch::BEST_LAP_GAP_BEHIND).value, Value::F64(1.5));
    assert_eq!(f.get(ch::CLASS_BEST_LAP_GAP_AHEAD).value, Value::F64(1.0));
    assert_eq!(f.get(ch::CLASS_BEST_LAP_GAP_BEHIND).value, Value::F64(1.5));
    for id in [
        ch::CLASS_GAP_AHEAD,
        ch::CLASS_GAP_BEHIND,
        ch::CLASS_GAP_AHEAD_LAPS,
        ch::CLASS_GAP_BEHIND_LAPS,
    ] {
        assert_eq!(quality(&f, id), Quality::Unavailable, "0x{:04X}", id.0);
    }
}

#[test]
fn out_of_range_values_are_invalid_not_clamped() {
    let mut f = frame();
    let mut r = PublishReport::default();
    let delta = Metric::derived(tyre::formula::DELTA_VS_STINT_START, Some(1.0e7));
    write(
        &mut f,
        ch::TYRE_PRESSURE_DELTA_FL,
        Out::float(delta),
        &mut r,
    );
    let laps = Gap {
        seconds: None,
        laps: Some(-1),
        other_slot_id: Some(1),
        provenance: Provenance::Estimated,
        formula: formula::GAP_FROM_LEADER_TIMES,
    };
    write(
        &mut f,
        ch::CLASS_GAP_AHEAD_LAPS,
        gap_part(&laps, formula::GAP_FROM_LEADER_TIMES, gap_laps),
        &mut r,
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(
        r.out_of_range,
        vec![ch::TYRE_PRESSURE_DELTA_FL, ch::CLASS_GAP_AHEAD_LAPS]
    );
    assert_eq!(quality(&f, ch::TYRE_PRESSURE_DELTA_FL), Quality::Invalid);
    assert_eq!(f.get(ch::TYRE_PRESSURE_DELTA_FL).value, Value::None);
}

#[test]
fn a_formula_or_provenance_that_differs_from_the_registry_is_reported() {
    let mut f = frame();
    let mut r = PublishReport::default();
    // Right value, wrong provenance (the rolling average is ESTIMATED in the registry).
    let wrong = Metric::derived(tyre::formula::ROLLING_AVG, Some(80.0));
    write(
        &mut f,
        ch::TYRE_TEMP_ROLLING_AVG_FL,
        Out::float(wrong),
        &mut r,
    );
    assert!(matches!(
        r.errors[..],
        [PublishError::ContractMismatch { channel, .. }] if channel == ch::TYRE_TEMP_ROLLING_AVG_FL.0
    ));
    assert_eq!(quality(&f, ch::TYRE_TEMP_ROLLING_AVG_FL), Quality::Invalid);
}

#[test]
fn band_and_flag_codes_fit_the_registry_ranges() {
    for b in [Band::Low, Band::InRange, Band::High] {
        let code = band_code(b).expect("code");
        for id in [ch::TYRE_TEMP_BAND_FL, ch::BRAKE_TEMP_BAND_RR] {
            let def = registry::by_id(id).expect("registered");
            assert_eq!(
                telemetry_model::validate::check_value(def, f64::from(code)),
                Ok(())
            );
        }
    }
    assert_eq!(band_code(Band::Unknown), None);
    let def = registry::by_id(ch::FLAG_SUMMARY).expect("registered");
    for flag in [
        RaceFlag::Green,
        RaceFlag::Yellow,
        RaceFlag::FullCourseYellow,
        RaceFlag::Blue,
        RaceFlag::Red,
        RaceFlag::Chequered,
    ] {
        let code = flag_summary_code(flag).expect("code");
        assert_eq!(
            telemetry_model::validate::check_value(def, f64::from(code)),
            Ok(())
        );
    }
    assert_eq!(flag_summary_code(RaceFlag::None), None);
}

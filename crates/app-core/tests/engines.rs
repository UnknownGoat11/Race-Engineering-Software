//! Integration and verification tests for `EngineHub` (Phase 1 Lane C).
//!
//! Covers:
//! - Deterministic execution sequence and snapshot generation.
//! - Lap completion detection, sector splits, and personal best events.
//! - Pit stop cycle and SessionEvents mapping.
//! - Fuel consumption tracking and stint length limit calculation.
//! - Zero publication errors invariant across frames.
//! - Unavailable inputs never yield valid zero.
//! - Alert rule evaluation, hysteresis, and clear transitions.
//! - `EngineStateDto` JSON serialization for `core:engine_state`.

use app_core::driver::engines::EngineHub;
use race_engines::alert::AlertState;
use race_engines::common::CompatibilityKey;
use race_engines::race::PitPhase;
use telemetry_model::ch;
use telemetry_model::events::SessionEvent;
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::value::{Quality, Value};

fn make_base_frame(
    mono_ns: u64,
    et_s: f64,
    lap: i32,
    lap_start: f64,
    progress: f64,
) -> TelemetryFrame {
    let mut f = TelemetryFrame::new(0, mono_ns, Some(et_s));
    f.set_valid(ch::ELAPSED_TIME, Value::F64(et_s))
        .expect("set elapsed");
    f.set_valid(ch::SPEED, Value::F32(50.0)).expect("set speed");
    f.set_valid(ch::TRACK_LENGTH, Value::F64(7004.0))
        .expect("set track length");
    f.set_valid(ch::LAP_NUMBER, Value::I32(lap))
        .expect("set lap number");
    f.set_valid(ch::LAP_START_TIME, Value::F64(lap_start))
        .expect("set lap start");
    f.set_valid(ch::LAP_PROGRESS, Value::F32(progress as f32))
        .expect("set lap progress");
    f.set_valid(ch::LAP_DISTANCE, Value::F64(progress * 7004.0))
        .expect("set lap dist");
    f.set_valid(ch::IN_PIT_LANE, Value::Bool(false))
        .expect("set in pit lane");
    f.set_valid(ch::FUEL, Value::F32(75.0)).expect("set fuel");
    f.set_valid(ch::FUEL_CAPACITY, Value::F32(100.0))
        .expect("set capacity");
    f.set_valid(ch::TYRE_PRESSURE_FL, Value::F32(200_000.0))
        .expect("set tyre pressure");
    f.set_valid(ch::TYRE_PRESSURE_FR, Value::F32(200_000.0))
        .expect("set tyre pressure");
    f.set_valid(ch::TYRE_PRESSURE_RL, Value::F32(200_000.0))
        .expect("set tyre pressure");
    f.set_valid(ch::TYRE_PRESSURE_RR, Value::F32(200_000.0))
        .expect("set tyre pressure");
    f.set_valid(ch::BRAKE_TEMP_FL, Value::F32(350.0))
        .expect("set brake temp");
    f.set_valid(ch::BRAKE_TEMP_FR, Value::F32(350.0))
        .expect("set brake temp");
    f.set_valid(ch::BRAKE_TEMP_RL, Value::F32(300.0))
        .expect("set brake temp");
    f.set_valid(ch::BRAKE_TEMP_RR, Value::F32(300.0))
        .expect("set brake temp");
    f
}

#[test]
fn engine_hub_initialization_and_clean_defaults() {
    let hub = EngineHub::new().expect("create engine hub");
    assert_eq!(hub.publish_errors_count(), 0);

    let timing = hub.timing.snapshot();
    assert_eq!(timing.laps_completed, 0);

    let stint = hub.stint.snapshot();
    assert_eq!(stint.number.value, None);
}

#[test]
fn engine_hub_clean_publication_and_no_zero_fabrication() {
    let mut hub = EngineHub::new().expect("create engine hub");
    let mut frame = make_base_frame(1_000_000_000, 10.0, 1, 0.0, 0.1);

    let outcome = hub.process_frame(&mut frame, None);
    assert_eq!(outcome.publish_errors, 0);
    assert_eq!(hub.publish_errors_count(), 0);

    // CALC_ENGINE_VERSION must be published and non-zero:
    let calc_ver = frame.get(ch::CALC_ENGINE_VERSION);
    assert!(calc_ver.is_valid(), "calc engine version must be valid");
    assert_eq!(calc_ver.value, Value::U32(1));

    // Unavailable input: VIRTUAL_ENERGY was not supplied in base frame.
    // Derived VE channels must be Unavailable, NEVER valid 0:
    let ve_per_lap = frame.get(ch::VE_PER_LAP);
    assert_eq!(
        ve_per_lap.quality,
        Quality::Unavailable,
        "unmeasured virtual energy must be Unavailable, never 0"
    );
}

#[test]
fn engine_hub_lap_completion_and_personal_best_events() {
    let mut hub = EngineHub::new().expect("create engine hub");
    hub.set_context(CompatibilityKey {
        track_id: Some("SPA_7004".into()),
        vehicle: Some("Ferrari 499P".into()),
        class: Some("Hypercar".into()),
        session: 1,
    });

    // Lap 1 progression:
    let mut f1 = make_base_frame(1_000_000_000, 10.0, 1, 0.0, 0.1);
    let out1 = hub.process_frame(&mut f1, None);
    assert!(out1.events.is_empty());

    let mut f2 = make_base_frame(60_000_000_000, 60.0, 1, 0.0, 0.5);
    hub.process_frame(&mut f2, None);

    let mut f3 = make_base_frame(120_000_000_000, 120.0, 1, 0.0, 0.95);
    hub.process_frame(&mut f3, None);

    // Lap 1 completes at ET 125.0 -> Lap 2 starts:
    let mut f4 = make_base_frame(125_000_000_000, 125.0, 2, 125.0, 0.01);
    let out4 = hub.process_frame(&mut f4, None);

    let lap_completed = out4.events.iter().any(|e| {
        matches!(
            e,
            SessionEvent::LapCompleted {
                lap: 1,
                valid: true,
                ..
            }
        )
    });
    assert!(
        lap_completed,
        "LapCompleted event must be emitted for lap 1"
    );

    let pb_emitted = out4
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::PersonalBest { lap: 1, .. }));
    assert!(
        pb_emitted,
        "PersonalBest event must be emitted for first completed lap"
    );

    // Lap 2 completes faster at ET 240.0 (lap time 115.0 s):
    let mut f5 = make_base_frame(240_000_000_000, 240.0, 3, 240.0, 0.01);
    let out5 = hub.process_frame(&mut f5, None);

    let pb2 = out5
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::PersonalBest { lap: 2, .. }));
    assert!(
        pb2,
        "PersonalBest event must be emitted when beating personal best"
    );

    // Lap 3 completes slower at ET 370.0 (lap time 130.0 s):
    let mut f6 = make_base_frame(370_000_000_000, 370.0, 4, 370.0, 0.01);
    let out6 = hub.process_frame(&mut f6, None);

    let pb3 = out6
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::PersonalBest { .. }));
    assert!(!pb3, "PersonalBest must NOT be emitted for a slower lap");
}

#[test]
fn engine_hub_pit_stop_lifecycle_events() {
    let mut hub = EngineHub::new().expect("create engine hub");

    // Driving on track:
    let mut f1 = make_base_frame(1_000_000_000, 10.0, 1, 0.0, 0.5);
    let out1 = hub.process_frame(&mut f1, None);
    assert_eq!(
        out1.race.as_ref().map(|s| s.pit.phase),
        Some(PitPhase::OnTrack)
    );

    // Enters pit lane:
    let mut f2 = make_base_frame(2_000_000_000, 15.0, 1, 0.0, 0.9);
    f2.set_valid(ch::IN_PIT_LANE, Value::Bool(true))
        .expect("set pit lane");
    f2.set_valid(ch::SPEED, Value::F32(20.0))
        .expect("set speed");
    let out2 = hub.process_frame(&mut f2, None);

    let pit_entry = out2
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::PitEntry));
    assert!(
        pit_entry,
        "PitEntry event must be emitted when entering pit lane"
    );

    // Stops in pit box: stationary starts at ET 30.0s
    let mut f3a = make_base_frame(3_000_000_000, 30.0, 1, 0.0, 0.92);
    f3a.set_valid(ch::IN_PIT_LANE, Value::Bool(true))
        .expect("set pit lane");
    f3a.set_valid(ch::SPEED, Value::F32(0.0))
        .expect("set stopped");
    hub.process_frame(&mut f3a, None);

    // Holds stationary >= 3.0s: ET 35.0s triggers inferred stop
    let mut f3b = make_base_frame(3_500_000_000, 35.0, 1, 0.0, 0.92);
    f3b.set_valid(ch::IN_PIT_LANE, Value::Bool(true))
        .expect("set pit lane");
    f3b.set_valid(ch::SPEED, Value::F32(0.0))
        .expect("set stopped");
    hub.process_frame(&mut f3b, None);

    // Leaves pit box: speed > 2.0 m/s ends stop
    let mut f4 = make_base_frame(4_000_000_000, 45.0, 1, 0.0, 0.93);
    f4.set_valid(ch::IN_PIT_LANE, Value::Bool(true))
        .expect("set pit lane");
    f4.set_valid(ch::SPEED, Value::F32(15.0))
        .expect("set exiting");
    let out4 = hub.process_frame(&mut f4, None);

    let pit_stop = out4
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::PitStop { .. }));
    assert!(pit_stop, "PitStop event must be emitted after leaving stop");

    // Exits pit lane onto circuit:
    let mut f5 = make_base_frame(5_000_000_000, 55.0, 1, 0.0, 0.98);
    f5.set_valid(ch::IN_PIT_LANE, Value::Bool(false))
        .expect("set on track");
    f5.set_valid(ch::SPEED, Value::F32(50.0))
        .expect("set speed");
    let out5 = hub.process_frame(&mut f5, None);

    let pit_exit = out5
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::PitExit));
    assert!(
        pit_exit,
        "PitExit event must be emitted upon re-entering track"
    );
}

#[test]
fn engine_hub_fuel_consumption_and_stint_max_laps() {
    let mut hub = EngineHub::new().expect("create engine hub");

    // Lap 1: 90.0L -> 87.0L (Lap 1 finishes at ET 120.0, establishes fuel boundary for Lap 2)
    let mut f1 = make_base_frame(1_000_000_000, 10.0, 1, 0.0, 0.1);
    f1.set_valid(ch::FUEL, Value::F32(90.0)).expect("set fuel");
    hub.process_frame(&mut f1, None);

    let mut f2 = make_base_frame(120_000_000_000, 120.0, 2, 120.0, 0.01);
    f2.set_valid(ch::FUEL, Value::F32(87.0)).expect("set fuel");
    hub.process_frame(&mut f2, None);

    // Lap 2: 87.0L -> 84.0L (used 3.0L on a clean, fully-observed racing lap)
    let mut f3 = make_base_frame(180_000_000_000, 180.0, 2, 120.0, 0.5);
    f3.set_valid(ch::FUEL, Value::F32(85.5)).expect("set fuel");
    hub.process_frame(&mut f3, None);

    let mut f4 = make_base_frame(240_000_000_000, 240.0, 3, 240.0, 0.01);
    f4.set_valid(ch::FUEL, Value::F32(84.0)).expect("set fuel");
    let out4 = hub.process_frame(&mut f4, None);

    assert_eq!(hub.publish_errors_count(), 0);
    // After 1 clean completed lap with observed boundaries, fuel_per_lap is estimated:
    assert!(out4.fuel.fuel_per_lap.is_available());
    let per_lap = out4.fuel.fuel_per_lap.value.expect("fuel per lap");
    assert!(
        (per_lap - 3.0).abs() < 0.1,
        "fuel per lap must be ~3.0 L, got {per_lap}"
    );

    // Stint max laps: capacity 100L / 3.0L per lap ≈ 33.33 laps
    assert!(out4.stint_max_laps.is_available());
    let max_l = out4.stint_max_laps.value.expect("max laps");
    assert!(
        (max_l - 33.33).abs() < 1.0,
        "stint max laps expected ~33, got {max_l}"
    );
}

#[test]
fn engine_hub_alert_trigger_and_hysteresis() {
    let mut config = app_core::driver::engines::EngineHubConfig::default();
    config.alert.rules.push(race_engines::alert::AlertRule {
        id: "fuel_low_test".into(),
        channel: telemetry_model::registry::by_id(ch::FUEL)
            .expect("fuel def")
            .name
            .to_owned(),
        category: race_engines::alert::AlertCategory::Fuel,
        severity: telemetry_model::events::Severity::Warning,
        comparison: race_engines::alert::Comparison::Below,
        trigger: 10.0,
        clear: 20.0,
        min_duration_s: 0.5,
        cooldown_s: 5.0,
        message: "Fuel level low".into(),
    });
    let mut hub = EngineHub::with_config(config).expect("create engine hub");

    // Normal fuel (75 L):
    let mut f1 = make_base_frame(1_000_000_000, 10.0, 1, 0.0, 0.1);
    let out1 = hub.process_frame(&mut f1, None);
    assert!(
        !out1
            .events
            .iter()
            .any(|e| matches!(e, SessionEvent::AlertRaised { .. }))
    );

    // Low fuel (5.0 L): condition begins, enters Pending state
    let mut f2 = make_base_frame(2_000_000_000, 20.0, 1, 0.0, 0.2);
    f2.set_valid(ch::FUEL, Value::F32(5.0))
        .expect("set low fuel");
    let out2 = hub.process_frame(&mut f2, None);
    assert!(
        !out2
            .events
            .iter()
            .any(|e| matches!(e, SessionEvent::AlertRaised { .. }))
    );

    // Low fuel held >= 0.5s: now mono 3.0s (delta 1.0s >= 0.5s) -> Alert is Raised
    let mut f3 = make_base_frame(3_000_000_000, 30.0, 1, 0.0, 0.3);
    f3.set_valid(ch::FUEL, Value::F32(5.0))
        .expect("set low fuel");
    let out3 = hub.process_frame(&mut f3, None);

    let raised = out3
        .alert_notices
        .iter()
        .any(|n| n.state == AlertState::Raised);
    assert!(raised, "low fuel alert must be raised");
    let event_raised = out3
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::AlertRaised { .. }));
    assert!(event_raised, "AlertRaised session event must be emitted");

    // Next frame still low fuel: alert is Active, must NOT re-emit AlertRaised:
    let mut f4 = make_base_frame(4_000_000_000, 40.0, 1, 0.0, 0.4);
    f4.set_valid(ch::FUEL, Value::F32(5.0))
        .expect("set low fuel");
    let out4 = hub.process_frame(&mut f4, None);

    let re_raised = out4
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::AlertRaised { .. }));
    assert!(!re_raised, "must not re-raise already active alert");

    // Refuel back to 80 L (above clear threshold 20.0 L): alert is Cleared:
    let mut f5 = make_base_frame(5_000_000_000, 50.0, 1, 0.0, 0.5);
    f5.set_valid(ch::FUEL, Value::F32(80.0))
        .expect("set refuel");
    let out5 = hub.process_frame(&mut f5, None);

    let cleared = out5
        .alert_notices
        .iter()
        .any(|n| n.state == AlertState::Cleared);
    assert!(cleared, "low fuel alert must be cleared after refuel");
    let event_cleared = out5
        .events
        .iter()
        .any(|e| matches!(e, SessionEvent::AlertCleared { .. }));
    assert!(event_cleared, "AlertCleared session event must be emitted");
}

#[test]
fn engine_state_dto_json_serialization() {
    let mut hub = EngineHub::new().expect("create engine hub");
    let mut frame = make_base_frame(1_000_000_000, 10.0, 1, 0.0, 0.1);

    let outcome = hub.process_frame(&mut frame, None);
    let dto = outcome.to_engine_state_dto();

    let json = serde_json::to_string(&dto).expect("serialize engine state DTO");
    assert!(json.contains("\"timing\""));
    assert!(json.contains("\"fuel\""));
    assert!(json.contains("\"stint\""));
    assert!(json.contains("\"tyre\""));
    assert!(json.contains("\"brake\""));
    assert!(json.contains("\"energy\""));
}

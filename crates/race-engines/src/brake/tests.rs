//! Brake engine tests (spec §291: thresholds, rate of change, wheel mapping).

use super::*;
use telemetry_model::Value;

fn sample(et: f64, t: [f64; WHEELS], bias: Option<f64>) -> BrakeSample {
    BrakeSample {
        et_s: et,
        temps_c: t.map(Some),
        bias_rear: bias,
    }
}

#[test]
fn from_frame_maps_wheels_and_bias() {
    let mut f = TelemetryFrame::new(1, 0, Some(3.0));
    f.set_valid(ch::BRAKE_TEMP_RL, Value::F32(450.0))
        .expect("set");
    f.set_valid(ch::BRAKE_BIAS_REAR, Value::F32(0.44))
        .expect("set");
    let s = BrakeSample::from_frame(&f).expect("sample");
    assert_eq!(s.temps_c[Wheel::RearLeft.index()], Some(450.0));
    assert_eq!(s.temps_c[Wheel::FrontLeft.index()], None);
    assert!((s.bias_rear.expect("bias") - 0.44).abs() < 1e-6);
}

#[test]
fn trend_peak_and_distribution() {
    let mut eng = BrakeEngine::new(BrakeConfig::default()).expect("config");
    for i in 0..20 {
        let t = f64::from(i) * 0.1;
        eng.update(&sample(
            t,
            [500.0 + 40.0 * t, 500.0, 300.0, 300.0],
            Some(0.45),
        ));
    }
    let s = eng.snapshot();
    let fl = s.wheels[0];
    assert!((fl.trend_c_per_s.value.expect("trend") - 40.0).abs() < 1e-6);
    assert!((fl.peak_c.value.expect("peak") - 576.0).abs() < 1e-9);
    assert!(s.front_rear_delta_c.value.expect("delta") > 200.0);
    assert!((s.bias_front.value.expect("bias") - 0.55).abs() < 1e-12);
}

#[test]
fn missing_bias_and_temps_are_unavailable() {
    let mut eng = BrakeEngine::new(BrakeConfig::default()).expect("config");
    eng.update(&BrakeSample {
        et_s: 0.0,
        temps_c: [Some(400.0), None, Some(300.0), Some(300.0)],
        bias_rear: None,
    });
    let s = eng.snapshot();
    assert!(!s.bias_rear.is_available());
    assert!(!s.bias_front.is_available());
    assert!(!s.front_rear_delta_c.is_available());
    assert!(!s.wheels[1].temp_c.is_available());
    assert_eq!(s.wheels[0].band, Band::Unknown);
}

#[test]
fn thresholds() {
    let cfg = BrakeConfig {
        thresholds: BrakeThresholds {
            temp_low_c: Some(200.0),
            temp_high_c: Some(800.0),
        },
        ..BrakeConfig::default()
    };
    let mut eng = BrakeEngine::new(cfg).expect("config");
    eng.update(&sample(0.0, [900.0, 500.0, 150.0, 800.0], None));
    let s = eng.snapshot();
    assert_eq!(s.wheels[0].band, Band::High);
    assert_eq!(s.wheels[1].band, Band::InRange);
    assert_eq!(s.wheels[2].band, Band::Low);
    assert_eq!(s.wheels[3].band, Band::InRange);
    let bad = BrakeConfig {
        trend_window_s: 0.0,
        ..BrakeConfig::default()
    };
    assert!(BrakeEngine::new(bad).is_err());
}

#[test]
fn snapshot_has_no_unit_dependent_temperature_ratio() {
    // Regression for review finding: a front/total ratio of Celsius temperatures changes with the unit.
    let mut eng = BrakeEngine::new(BrakeConfig::default()).expect("config");
    eng.update(&sample(0.0, [400.0, 400.0, 200.0, 200.0], Some(0.45)));
    let json = serde_json::to_value(eng.snapshot()).expect("serialize");
    let keys: Vec<&String> = json.as_object().expect("object").keys().collect();
    assert!(
        keys.iter().all(|k| !k.contains("share")),
        "unexpected ratio metric in {keys:?}"
    );
    assert_eq!(eng.snapshot().front_rear_delta_c.value, Some(200.0));
}

/// Regression: the peak since the stint start survived a session restart (elapsed-time regression).
#[test]
fn time_regression_restarts_the_stint_peak() {
    let mut eng = BrakeEngine::new(BrakeConfig::default()).expect("config");
    eng.update(&sample(500.0, [950.0, 900.0, 700.0, 700.0], None));
    eng.update(&sample(501.0, [600.0, 600.0, 500.0, 500.0], None));
    assert_eq!(eng.snapshot().wheels[0].peak_c.value, Some(950.0));
    // Next session: elapsed time restarts, no pit exit (start_stint) yet.
    eng.update(&sample(10.0, [300.0, 310.0, 280.0, 290.0], None));
    let s = eng.snapshot();
    assert_eq!(s.wheels[0].peak_c.value, Some(300.0));
    assert_eq!(s.wheels[1].peak_c.value, Some(310.0));
    assert_eq!(s.wheels[0].trend_c_per_s.value, None);
}

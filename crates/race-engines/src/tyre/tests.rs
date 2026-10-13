//! Tyre engine tests (spec §291: rolling average, thresholds, rate of change, wheel mapping).

use super::*;
use telemetry_model::{Provenance, Value};

const EPS: f64 = 1e-9;

fn wheel(left: f64, mid: f64, right: f64, pressure: f64) -> TyreWheelSample {
    TyreWheelSample {
        pressure_pa: Some(pressure),
        surface_left_c: Some(left),
        surface_center_c: Some(mid),
        surface_right_c: Some(right),
        layer_left_c: Some(left),
        layer_center_c: Some(mid),
        layer_right_c: Some(right),
        carcass_c: Some(mid),
    }
}

fn uniform(et: f64, temp: f64, pressure: f64) -> TyreSample {
    TyreSample {
        et_s: et,
        wheels: [wheel(temp, temp, temp, pressure); WHEELS],
    }
}

fn engine(cfg: TyreConfig) -> TyreEngine {
    TyreEngine::new(cfg).expect("valid config")
}

#[test]
fn wheel_mapping_inner_outer_by_side() {
    // Car-frame left edge 90, centre 85, right edge 80 on every wheel.
    let mut eng = engine(TyreConfig::default());
    eng.update(&TyreSample {
        et_s: 1.0,
        wheels: [wheel(90.0, 85.0, 80.0, 170_000.0); WHEELS],
    });
    let s = eng.snapshot();
    for w in [Wheel::FrontLeft, Wheel::RearLeft] {
        let st = s.wheels[w.index()];
        assert_eq!(
            st.inner_c.value,
            Some(80.0),
            "{} inner is the right edge",
            w.label()
        );
        assert_eq!(st.outer_c.value, Some(90.0));
        assert_eq!(st.inner_middle_delta_c.value, Some(-5.0));
        assert_eq!(st.middle_outer_delta_c.value, Some(-5.0));
    }
    for w in [Wheel::FrontRight, Wheel::RearRight] {
        let st = s.wheels[w.index()];
        assert_eq!(
            st.inner_c.value,
            Some(90.0),
            "{} inner is the left edge",
            w.label()
        );
        assert_eq!(st.outer_c.value, Some(80.0));
        assert_eq!(st.inner_middle_delta_c.value, Some(5.0));
    }
    assert_eq!(s.wheels[0].inner_c.formula, formula::EDGE_MAPPING);
}

#[test]
fn from_frame_maps_channels_per_wheel() {
    let mut f = TelemetryFrame::new(1, 0, Some(12.5));
    f.set_valid(ch::TYRE_TEMP_LEFT_RR, Value::F32(70.0))
        .expect("set");
    f.set_valid(ch::TYRE_TEMP_MIDDLE_RR, Value::F32(75.0))
        .expect("set");
    f.set_valid(ch::TYRE_TEMP_RIGHT_RR, Value::F32(80.0))
        .expect("set");
    f.set_valid(ch::TYRE_PRESSURE_FL, Value::F32(165_000.0))
        .expect("set");
    let s = TyreSample::from_frame(&f).expect("sample");
    assert_eq!(s.et_s, 12.5);
    let rr = s.wheels[Wheel::RearRight.index()];
    assert_eq!(rr.surface_left_c, Some(70.0));
    assert_eq!(rr.surface_right_c, Some(80.0));
    assert_eq!(s.wheels[0].pressure_pa, Some(165_000.0));
    // Unavailable channels are None, never 0.
    assert_eq!(s.wheels[0].surface_left_c, None);
    assert_eq!(rr.pressure_pa, None);
}

#[test]
fn missing_inputs_are_unavailable_not_zero() {
    let mut eng = engine(TyreConfig::default());
    eng.update(&TyreSample {
        et_s: 0.0,
        wheels: [TyreWheelSample::default(); WHEELS],
    });
    let s = eng.snapshot();
    let w = s.wheels[0];
    assert!(!w.surface_avg_c.is_available());
    assert!(!w.pressure_delta_pa.is_available());
    assert!(!w.rolling_avg_surface_c.is_available());
    assert!(!s.front_rear_temp_balance_c.is_available());
    assert_eq!(w.surface_band, Band::Unknown);
}

#[test]
fn rolling_average_over_time_window() {
    let cfg = TyreConfig {
        rolling_window_s: 2.0,
        ..TyreConfig::default()
    };
    let mut eng = engine(cfg);
    // Temps 80, 81, ... at 1 s steps; at t=10 the window [8,10] holds 88, 89, 90.
    for i in 0..=10 {
        eng.update(&uniform(f64::from(i), 80.0 + f64::from(i), 170_000.0));
    }
    let w = eng.snapshot().wheels[0];
    assert!((w.rolling_avg_surface_c.value.expect("avg") - 89.0).abs() < EPS);
    assert_eq!(w.rolling_avg_surface_c.provenance, Provenance::Estimated);
    assert_eq!(w.peak_surface_c.value, Some(90.0));
    assert_eq!(w.surface_delta_vs_start_c.value, Some(10.0));
}

#[test]
fn rise_rate_is_slope() {
    let mut eng = engine(TyreConfig::default());
    for i in 0..30 {
        let t = f64::from(i) * 0.1;
        eng.update(&uniform(t, 60.0 + 2.5 * t, 170_000.0));
    }
    let rate = eng.snapshot().wheels[3]
        .rise_rate_c_per_s
        .value
        .expect("rate");
    assert!((rate - 2.5).abs() < 1e-6, "rate {rate}");
}

#[test]
fn time_regression_clears_windows() {
    let mut eng = engine(TyreConfig::default());
    for i in 0..5 {
        eng.update(&uniform(100.0 + f64::from(i), 100.0, 170_000.0));
    }
    eng.update(&uniform(1.0, 50.0, 170_000.0));
    let w = eng.snapshot().wheels[0];
    assert_eq!(w.rolling_avg_surface_c.value, Some(50.0));
    assert!(w.rise_rate_c_per_s.value.is_none());
}

#[test]
fn time_regression_restarts_stint_start_values_and_peak() {
    // Regression for review finding: a session restart must not compare against the old session's tyres.
    let mut eng = engine(TyreConfig::default());
    eng.update(&uniform(500.0, 60.0, 160_000.0));
    eng.update(&uniform(501.0, 110.0, 185_000.0));
    eng.update(&uniform(2.0, 40.0, 150_000.0));
    let w = eng.snapshot().wheels[2];
    assert_eq!(w.start_pressure_pa.value, Some(150_000.0));
    assert_eq!(w.pressure_delta_pa.value, Some(0.0));
    assert_eq!(w.peak_surface_c.value, Some(40.0));
    assert_eq!(w.surface_delta_vs_start_c.value, Some(0.0));
    assert_eq!(w.carcass_delta_vs_start_c.value, Some(0.0));
}

#[test]
fn pressure_delta_and_stint_reset() {
    let mut eng = engine(TyreConfig::default());
    eng.update(&uniform(0.0, 60.0, 160_000.0));
    eng.update(&uniform(1.0, 70.0, 175_000.0));
    let w = eng.snapshot().wheels[1];
    assert_eq!(w.start_pressure_pa.value, Some(160_000.0));
    assert_eq!(w.pressure_delta_pa.value, Some(15_000.0));
    eng.start_stint();
    eng.update(&uniform(2.0, 50.0, 150_000.0));
    let w = eng.snapshot().wheels[1];
    assert_eq!(w.start_pressure_pa.value, Some(150_000.0));
    assert_eq!(w.pressure_delta_pa.value, Some(0.0));
    assert_eq!(w.peak_surface_c.value, Some(50.0));
}

#[test]
fn balances() {
    let mut eng = engine(TyreConfig::default());
    eng.update(&TyreSample {
        et_s: 0.0,
        wheels: [
            wheel(90.0, 90.0, 90.0, 170_000.0), // FL
            wheel(80.0, 80.0, 80.0, 172_000.0), // FR
            wheel(70.0, 70.0, 70.0, 168_000.0), // RL
            wheel(60.0, 60.0, 60.0, 166_000.0), // RR
        ],
    });
    let s = eng.snapshot();
    assert_eq!(s.front_rear_temp_balance_c.value, Some(20.0));
    assert_eq!(s.left_right_temp_balance_c.value, Some(10.0));
    assert_eq!(s.front_rear_pressure_balance_pa.value, Some(4_000.0));
    assert_eq!(s.left_right_pressure_balance_pa.value, Some(0.0));
}

#[test]
fn thresholds_classify_and_default_is_unknown() {
    let mut eng = engine(TyreConfig::default());
    eng.update(&uniform(0.0, 120.0, 200_000.0));
    assert_eq!(eng.snapshot().wheels[0].surface_band, Band::Unknown);

    let cfg = TyreConfig {
        thresholds: TyreThresholds {
            surface_low_c: Some(70.0),
            surface_high_c: Some(100.0),
            pressure_low_pa: Some(160_000.0),
            pressure_high_pa: Some(180_000.0),
            edge_spread_max_c: Some(8.0),
        },
        ..TyreConfig::default()
    };
    let mut eng = engine(cfg);
    eng.update(&TyreSample {
        et_s: 0.0,
        wheels: [
            wheel(120.0, 120.0, 120.0, 150_000.0),
            wheel(60.0, 60.0, 60.0, 190_000.0),
            wheel(80.0, 85.0, 90.0, 170_000.0),
            wheel(70.0, 85.0, 100.0, 170_000.0),
        ],
    });
    let s = eng.snapshot();
    assert_eq!(s.wheels[0].surface_band, Band::High);
    assert_eq!(s.wheels[0].pressure_band, Band::Low);
    assert_eq!(s.wheels[1].surface_band, Band::Low);
    assert_eq!(s.wheels[1].pressure_band, Band::High);
    assert_eq!(s.wheels[2].surface_band, Band::InRange);
    assert_eq!(s.wheels[2].edge_spread_band, Band::High);
    assert_eq!(s.wheels[3].edge_spread_band, Band::High);
}

#[test]
fn config_validation() {
    assert!(TyreConfig::default().validate().is_ok());
    let bad = TyreConfig {
        rolling_window_s: -1.0,
        ..TyreConfig::default()
    };
    assert!(matches!(
        bad.validate(),
        Err(TyreConfigError::InvalidWindow { .. })
    ));
    let bad = TyreConfig {
        thresholds: TyreThresholds {
            surface_low_c: Some(100.0),
            surface_high_c: Some(90.0),
            ..TyreThresholds::default()
        },
        ..TyreConfig::default()
    };
    assert!(TyreEngine::new(bad).is_err());
    let bad = TyreConfig {
        thresholds: TyreThresholds {
            pressure_low_pa: Some(-1.0),
            ..TyreThresholds::default()
        },
        ..TyreConfig::default()
    };
    assert!(bad.validate().is_err());
}

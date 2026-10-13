//! Alert engine tests (spec §291 hysteresis; §31/§70 one continuous condition → one alert).

use super::*;
use telemetry_model::Value;

const DT: f64 = 0.01;

fn rule(min_duration_s: f64, cooldown_s: f64) -> AlertRule {
    AlertRule {
        id: "brake_hot".into(),
        channel: "brakes.fl.temp_c".into(),
        category: AlertCategory::Brakes,
        severity: Severity::Warning,
        comparison: Comparison::Above,
        trigger: 800.0,
        clear: 750.0,
        min_duration_s,
        cooldown_s,
        message: "FL brake hot".into(),
    }
}

fn engine_with(rules: Vec<AlertRule>) -> AlertEngine {
    AlertEngine::new(&AlertConfig {
        include_default_rules: false,
        rules,
        ..AlertConfig::default()
    })
    .expect("config")
}

/// Runs values at DT steps from `t0`; returns (notices, end time).
fn run(eng: &mut AlertEngine, t0: f64, values: &[f64]) -> (Vec<AlertNotice>, f64) {
    let mut out = Vec::new();
    let mut t = t0;
    for v in values {
        out.extend(eng.evaluate(t, Some(3), |_| Some(*v)));
        t += DT;
    }
    (out, t)
}

fn count(n: &[AlertNotice], state: AlertState) -> usize {
    n.iter().filter(|x| x.state == state).count()
}

#[test]
fn condition_held_250_samples_gives_one_alert() {
    let mut eng = engine_with(vec![rule(0.5, 10.0)]);
    let (n, _) = run(&mut eng, 0.0, &[850.0; 250]);
    assert_eq!(count(&n, AlertState::Raised), 1);
    assert_eq!(eng.active().len(), 1);
    assert_eq!(eng.anomalies().count(), 1);
    let a = &eng.active()[0];
    assert_eq!(a.state, AlertState::Active);
    // Raised after the 0.5 s minimum duration.
    assert!((a.raised_s - 0.5).abs() < 1e-9);
    let rec = eng.anomalies().next().expect("anomaly");
    assert_eq!(rec.measured, 850.0);
    assert_eq!(rec.threshold, 800.0);
    assert_eq!(rec.lap, Some(3));
    assert_eq!(rec.channel, "brakes.fl.temp_c");
}

#[test]
fn flapping_inside_hysteresis_does_not_spam() {
    let mut eng = engine_with(vec![rule(0.0, 0.0)]);
    let values: Vec<f64> = (0..500)
        .map(|i| if i % 2 == 0 { 801.0 } else { 790.0 })
        .collect();
    let (n, _) = run(&mut eng, 0.0, &values);
    assert_eq!(count(&n, AlertState::Raised), 1);
    assert_eq!(count(&n, AlertState::Cleared), 0);
}

#[test]
fn flapping_before_min_duration_never_raises() {
    let mut eng = engine_with(vec![rule(0.1, 0.0)]);
    let values: Vec<f64> = (0..500)
        .map(|i| if i % 5 == 0 { 790.0 } else { 820.0 })
        .collect();
    let (n, _) = run(&mut eng, 0.0, &values);
    assert!(n.is_empty());
}

#[test]
fn clear_then_cooldown_suppresses_reraise() {
    let mut eng = engine_with(vec![rule(0.0, 1.0)]);
    let (n, t) = run(&mut eng, 0.0, &[850.0; 10]);
    assert_eq!(count(&n, AlertState::Raised), 1);
    let (n, t) = run(&mut eng, t, &[700.0]);
    assert_eq!(count(&n, AlertState::Cleared), 1);
    assert!(eng.active().is_empty());
    assert_eq!(eng.history().count(), 1);
    // Within the 1 s cooldown: no new alert.
    let (n, t) = run(&mut eng, t, &[900.0; 50]);
    assert!(n.is_empty());
    // After the cooldown: a new alert.
    let (n, _) = run(&mut eng, t + 1.0, &[900.0; 2]);
    assert_eq!(count(&n, AlertState::Raised), 1);
}

#[test]
fn missing_value_holds_active_and_drops_pending() {
    let mut eng = engine_with(vec![rule(0.0, 0.0)]);
    run(&mut eng, 0.0, &[850.0]);
    let n = eng.evaluate(1.0, None, |_| None);
    assert!(n.is_empty());
    assert_eq!(eng.active().len(), 1);

    let mut eng = engine_with(vec![rule(1.0, 0.0)]);
    run(&mut eng, 0.0, &[850.0; 50]);
    eng.evaluate(0.6, None, |_| None);
    let (n, _) = run(&mut eng, 0.7, &[850.0; 50]);
    assert!(n.is_empty(), "pending restarted after the missing value");
}

#[test]
fn below_rule_and_acknowledge() {
    let r = AlertRule {
        id: "low".into(),
        channel: "x".into(),
        comparison: Comparison::Below,
        trigger: 2.0,
        clear: 2.5,
        ..rule(0.0, 0.0)
    };
    let mut eng = engine_with(vec![r]);
    let (n, _) = run(&mut eng, 0.0, &[1.9, 2.1, 1.5]);
    assert_eq!(count(&n, AlertState::Raised), 1);
    let id = eng.active()[0].id;
    assert_eq!(eng.active()[0].worst_value, 1.5);
    eng.acknowledge(id).expect("ack");
    assert!(eng.active()[0].acknowledged);
    assert_eq!(eng.acknowledge(999), Err(AlertError::UnknownAlert(999)));
    let (n, _) = run(&mut eng, 1.0, &[2.6]);
    assert_eq!(count(&n, AlertState::Cleared), 1);
}

#[test]
fn several_rules_clear_independently() {
    let mut r2 = rule(0.0, 0.0);
    r2.id = "other".into();
    r2.channel = "y".into();
    let mut eng = engine_with(vec![rule(0.0, 0.0), r2]);
    eng.evaluate(0.0, None, |_| Some(900.0));
    assert_eq!(eng.active().len(), 2);
    let n = eng.evaluate(0.1, None, |c| Some(if c == "y" { 900.0 } else { 700.0 }));
    assert_eq!(count(&n, AlertState::Cleared), 1);
    assert_eq!(eng.active()[0].rule_id, "other");
    let n = eng.evaluate(0.2, None, |_| Some(700.0));
    assert_eq!(n[0].alert.rule_id, "other");
}

#[test]
fn default_rules_and_frame_evaluation() {
    let mut eng = AlertEngine::new(&AlertConfig::default()).expect("defaults");
    assert!(
        eng.rules()
            .iter()
            .any(|r| r.channel == "fuel.laps_remaining")
    );
    let mut f = TelemetryFrame::new(1, 0, Some(10.0));
    f.set_valid(ch::FUEL_LAPS_REMAINING, Value::F32(1.5))
        .expect("set");
    assert!(eng.evaluate_frame(10.0, Some(5), &f).is_empty());
    let n = eng.evaluate_frame(11.0, Some(5), &f);
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].alert.category, AlertCategory::Fuel);
}

#[test]
fn config_validation() {
    let bad = AlertConfig {
        fuel_reserve_laps: -1.0,
        ..AlertConfig::default()
    };
    assert!(matches!(
        bad.validate(),
        Err(AlertConfigError::InvalidParameter {
            field: "fuel_reserve_laps",
            ..
        })
    ));
    let mut r = rule(0.0, 0.0);
    r.clear = 900.0; // above trigger on an Above rule
    assert!(r.validate().is_err());
    let r = rule(-1.0, 0.0);
    assert!(r.validate().is_err());
    let dup = AlertConfig {
        include_default_rules: false,
        rules: vec![rule(0.0, 0.0), rule(0.0, 0.0)],
        ..AlertConfig::default()
    };
    assert!(matches!(
        dup.validate(),
        Err(AlertConfigError::DuplicateRule(_))
    ));
    // Serde round trip with defaults filled in.
    let json = r#"{"fuel_reserve_laps": 1.5}"#;
    let cfg: AlertConfig = serde_json::from_str(json).expect("parse");
    assert!(cfg.validate().is_ok());
    assert!(cfg.include_default_rules);
}

#[test]
fn every_default_rule_watches_a_registry_channel() {
    // Regression for review finding: a default rule on a name nobody can write would silently never fire.
    for r in AlertConfig::default().default_rules() {
        assert!(
            telemetry_model::registry::by_name(&r.channel).is_some(),
            "rule {} watches unknown channel {}",
            r.id,
            r.channel
        );
    }
}

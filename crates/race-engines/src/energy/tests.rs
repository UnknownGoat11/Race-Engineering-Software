//! Energy engine tests (spec Â§291, Â§314: unavailable on non-hybrid, no battery substitution).

use super::*;
use telemetry_model::{Provenance, Value};

const EPS: f64 = 1e-9;
const DT: f64 = 0.1;
const SAMPLES_PER_LAP: usize = 100;

fn engine() -> EnergyEngine {
    EnergyEngine::new(EnergyConfig::default()).expect("config")
}

fn hybrid(et: f64, ve: f64, soc: f64, torque: f64, progress: f64) -> EnergySample {
    EnergySample {
        et_s: et,
        virtual_energy: Some(ve),
        soc: Some(soc),
        regen_power_w: Some(0.0),
        motor_torque_nm: Some(torque),
        motor_rpm: Some(6_000.0),
        lap_progress: Some(progress),
    }
}

/// Drives `laps` laps using `ve_per_lap` VE each; returns the final et.
fn drive(
    eng: &mut EnergyEngine,
    start_et: f64,
    start_ve: f64,
    ve_per_lap: f64,
    laps: usize,
) -> f64 {
    let mut et = start_et;
    let mut ve = start_ve;
    let per_sample = ve_per_lap / SAMPLES_PER_LAP as f64;
    for _ in 0..laps {
        for i in 0..SAMPLES_PER_LAP {
            et += DT;
            ve -= per_sample;
            let p = (i + 1) as f64 / SAMPLES_PER_LAP as f64;
            eng.update(&hybrid(et, ve, 0.5, 100.0, p));
        }
        eng.lap_completed(true);
    }
    et
}

#[test]
fn non_hybrid_is_unavailable_and_never_substitutes_battery() {
    let mut eng = engine();
    for i in 0..50 {
        eng.update(&EnergySample {
            et_s: f64::from(i) * DT,
            // A battery fraction exists but no virtual energy.
            soc: Some(0.8 - 0.001 * f64::from(i)),
            ..EnergySample::default()
        });
    }
    eng.lap_completed(true);
    let s = eng.snapshot();
    for m in [
        s.virtual_energy,
        s.ve_used_this_lap,
        s.ve_used_last_lap,
        s.ve_per_lap,
        s.ve_laps_remaining,
        s.ve_at_lap_end,
        s.ve_rate_per_s,
        s.motor_power_w,
        s.deployed_this_lap_j,
        s.regen_this_lap_j,
    ] {
        assert!(!m.is_available(), "{} must be unavailable", m.formula);
    }
    assert_eq!(s.last_lap.expect("lap").ve_used, None);
    assert!(s.soc.is_available());
}

#[test]
fn non_hybrid_frame_gives_none_channels() {
    let mut f = TelemetryFrame::new(1, 0, Some(5.0));
    f.set_valid(ch::BATTERY_SOC, Value::F32(0.6)).expect("set");
    let s = EnergySample::from_frame(&f, None).expect("sample");
    assert_eq!(s.virtual_energy, None);
    assert_eq!(s.motor_torque_nm, None);
    assert!(s.soc.is_some());
}

#[test]
fn ve_per_lap_and_lap_end_estimate() {
    let mut eng = engine();
    eng.update(&hybrid(0.0, 1.0, 0.5, 100.0, 0.0));
    drive(&mut eng, 0.0, 1.0, 0.05, 3);
    let s = eng.snapshot();
    let last = s.ve_used_last_lap.value.expect("last lap");
    assert!((last - 0.05).abs() < 1e-9, "{last}");
    assert!((s.ve_per_lap.value.expect("avg") - 0.05).abs() < 1e-9);
    assert_eq!(s.ve_per_lap.provenance, Provenance::Estimated);
    let ve = s.virtual_energy.value.expect("ve");
    assert!((ve - 0.85).abs() < 1e-9);
    assert!((s.ve_laps_remaining.value.expect("laps") - 17.0).abs() < 1e-6);
    // Progress 1.0 at the boundary sample: nothing left to use this lap.
    assert!((s.ve_at_lap_end.value.expect("end") - ve).abs() < EPS);
    // Consumption rate: 0.05 per 10 s lap.
    assert!((s.ve_rate_per_s.value.expect("rate") - 0.005).abs() < 1e-9);
}

#[test]
fn refill_never_makes_consumption_negative() {
    let mut eng = engine();
    eng.update(&hybrid(0.0, 0.3, 0.5, 0.0, 0.0));
    eng.update(&hybrid(0.1, 0.29, 0.5, 0.0, 0.1));
    eng.update(&hybrid(0.2, 1.0, 0.5, 0.0, 0.2)); // pit refill step
    eng.update(&hybrid(0.3, 0.98, 0.5, 0.0, 0.3));
    let rec = eng.lap_completed(false);
    assert!((rec.ve_used.expect("used") - 0.03).abs() < 1e-9);
}

#[test]
fn motor_deploy_and_harvest_split_by_sign() {
    let mut eng = engine();
    // +100 NÂ·m for 1 s then âˆ’100 NÂ·m for 1 s at 6000 rpm.
    let omega = 6_000.0 * 2.0 * PI / 60.0;
    for i in 0..=10 {
        eng.update(&hybrid(f64::from(i) * DT, 1.0, 0.5, 100.0, 0.0));
    }
    for i in 11..=20 {
        eng.update(&hybrid(f64::from(i) * DT, 1.0, 0.5, -100.0, 0.0));
    }
    let s = eng.snapshot();
    let dep = s.deployed_this_lap_j.value.expect("dep");
    let har = s.harvested_this_lap_j.value.expect("har");
    // The 10â†’11 interval is split half/half by the trapezoid.
    let expected = 100.0 * omega * (1.0 + DT / 2.0);
    assert!((dep - expected).abs() < 1e-6, "{dep} vs {expected}");
    assert!(
        (har - 100.0 * omega * (0.9 + DT / 2.0)).abs() < 1e-6,
        "{har}"
    );
    assert!(s.motor_power_w.value.expect("p") < 0.0);
}

#[test]
fn soc_deployed_and_harvested() {
    let mut eng = engine();
    let socs = [0.5, 0.48, 0.46, 0.47, 0.49, 0.45];
    for (i, soc) in socs.iter().enumerate() {
        eng.update(&hybrid(i as f64 * DT, 1.0, *soc, 0.0, 0.0));
    }
    let s = eng.snapshot();
    assert!((s.soc_deployed_this_lap.value.expect("d") - 0.08).abs() < 1e-9);
    assert!((s.soc_harvested_this_lap.value.expect("h") - 0.03).abs() < 1e-9);
}

#[test]
fn gap_makes_lap_incomplete_and_excluded_from_average() {
    let mut eng = engine();
    eng.update(&hybrid(0.0, 1.0, 0.5, 0.0, 0.0));
    eng.update(&hybrid(0.1, 0.99, 0.5, 0.0, 0.1));
    eng.update(&hybrid(5.0, 0.95, 0.5, 0.0, 0.5)); // 4.9 s gap
    let rec = eng.lap_completed(true);
    assert_eq!(rec.ve_used, None);
    assert!(!eng.snapshot().ve_per_lap.is_available());
}

#[test]
fn excluded_laps_do_not_feed_average() {
    let mut eng = engine();
    eng.update(&hybrid(0.0, 1.0, 0.5, 0.0, 0.0));
    eng.update(&hybrid(0.1, 0.96, 0.5, 0.0, 1.0));
    eng.lap_completed(false);
    assert!(!eng.snapshot().ve_per_lap.is_available());
    assert!(eng.snapshot().ve_used_last_lap.is_available());
}

#[test]
fn config_validation() {
    assert!(
        EnergyEngine::new(EnergyConfig {
            ve_window_laps: 0,
            ..EnergyConfig::default()
        })
        .is_err()
    );
    assert!(
        EnergyConfig {
            rate_window_s: f64::NAN,
            ..EnergyConfig::default()
        }
        .validate()
        .is_err()
    );
}

// ---- Regression tests for the enginesB review findings ----

fn regen_sample(et: f64, regen_w: f64) -> EnergySample {
    EnergySample {
        et_s: et,
        regen_power_w: Some(regen_w),
        lap_progress: Some(0.0),
        ..EnergySample::default()
    }
}

#[test]
fn negative_regen_is_counted_not_a_silent_zero() {
    // mRegen sign is unverified: -150 kW for 1 s must give 150 kJ, the same as +150 kW.
    for sign in [-1.0, 1.0] {
        let mut eng = engine();
        for i in 0..=10 {
            eng.update(&regen_sample(f64::from(i) * DT, sign * 150_000.0));
        }
        let live = eng.snapshot().regen_this_lap_j.value.expect("live regen");
        assert!((live - 150_000.0).abs() < 1e-6, "sign {sign}: {live}");
        let rec = eng.lap_completed(true);
        let lap = rec.regen_j.expect("lap regen");
        assert!((lap - 150_000.0).abs() < 1e-6, "sign {sign}: {lap}");
    }
}

#[test]
fn ve_refill_clears_rate_window() {
    let mut eng = engine();
    let mut et = 0.0;
    let mut ve = 0.12;
    for _ in 0..20 {
        eng.update(&hybrid(et, ve, 0.5, 0.0, 0.0));
        et += DT;
        ve -= 0.001;
    }
    // Pit refill step.
    eng.update(&hybrid(et, 1.0, 0.5, 0.0, 0.0));
    assert!(
        !eng.snapshot().ve_rate_per_s.is_available(),
        "the refill step must not produce a rate"
    );
    for i in 1..=5 {
        eng.update(&hybrid(
            et + f64::from(i) * DT,
            1.0 - 0.001 * f64::from(i),
            0.5,
            0.0,
            0.0,
        ));
    }
    let rate = eng.snapshot().ve_rate_per_s.value.expect("rate");
    assert!((rate - 0.01).abs() < 1e-9, "{rate}");
}

#[test]
fn soc_jump_clears_rate_window() {
    let mut eng = engine();
    for i in 0..10 {
        eng.update(&hybrid(f64::from(i) * DT, 1.0, 0.30, 0.0, 0.0));
    }
    eng.update(&hybrid(1.0, 1.0, 0.90, 0.0, 0.0));
    assert!(!eng.snapshot().soc_rate_per_s.is_available());
}

#[test]
fn duplicate_lap_boundary_is_not_a_complete_zero_lap() {
    let mut eng = engine();
    eng.update(&hybrid(0.0, 1.0, 0.5, 0.0, 0.0));
    eng.update(&hybrid(0.1, 0.99, 0.5, 0.0, 1.0));
    let first = eng.lap_completed(true);
    assert!((first.ve_used.expect("first") - 0.01).abs() < 1e-12);
    // A second boundary with no new sample: nothing was integrated.
    let dup = eng.lap_completed(true);
    assert_eq!(dup.ve_used, None);
    assert_eq!(dup.soc_deployed, None);
    assert_eq!(dup.regen_j, None);
    assert!(!eng.snapshot().ve_used_last_lap.is_available());
    // The duplicate never drags the average down.
    assert!((eng.snapshot().ve_per_lap.value.expect("avg") - 0.01).abs() < 1e-12);
}

#[test]
fn first_lap_joined_part_way_is_incomplete() {
    for progress in [Some(0.5), None] {
        let mut eng = engine();
        let mut s = hybrid(0.0, 0.8, 0.5, 0.0, 0.5);
        s.lap_progress = progress;
        eng.update(&s);
        s.et_s = 0.1;
        s.virtual_energy = Some(0.79);
        eng.update(&s);
        let rec = eng.lap_completed(true);
        assert_eq!(rec.ve_used, None, "progress {progress:?}");
        assert!(!eng.snapshot().ve_used_last_lap.is_available());
        // The next lap starts at a marked boundary and is complete.
        s.et_s = 0.2;
        s.virtual_energy = Some(0.78);
        eng.update(&s);
        let rec = eng.lap_completed(true);
        assert!((rec.ve_used.expect("second lap") - 0.01).abs() < 1e-9);
    }
}

#[test]
fn publish_energy_writes_ve_channels_and_unavailable_never_zero() {
    let mut eng = engine();
    eng.update(&hybrid(0.0, 1.0, 0.5, 100.0, 0.0));
    drive(&mut eng, 0.0, 1.0, 0.05, 3);
    let snap = eng.snapshot();
    let mut f = TelemetryFrame::new(1, 0, Some(30.0));
    let r = publish_energy(&mut f, Some(&snap));
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    // Right at the boundary nothing of the new lap is integrated yet: this-lap is Unavailable, not 0.
    assert_eq!(r.valid, PUBLISHED_CHANNELS.len() - 1);
    assert_eq!(
        f.get(ch::VE_USED_THIS_LAP).quality,
        telemetry_model::Quality::Unavailable
    );
    let laps = f.f64(ch::VE_LAPS_REMAINING).expect("laps remaining");
    assert!((laps - 17.0).abs() < 1e-4, "{laps}");
    let per_lap = f.f64(ch::VE_PER_LAP).expect("per lap");
    assert!((per_lap - 0.05).abs() < 1e-6);

    let mut f = TelemetryFrame::new(2, 0, Some(31.0));
    let r = publish_energy(&mut f, None);
    assert_eq!(r.unavailable, PUBLISHED_CHANNELS.len());
    for id in PUBLISHED_CHANNELS {
        assert_eq!(f.get(*id).quality, telemetry_model::Quality::Unavailable);
    }
}

#[test]
fn energy_low_default_alert_fires_end_to_end() {
    use crate::alert::{AlertClock, AlertConfig, AlertEngine, AlertState};
    // 0.1 VE per lap from 0.6: 0.1 left = 1 lap, below the default 2-lap reserve.
    let mut eng = engine();
    eng.update(&hybrid(0.0, 0.6, 0.5, 0.0, 0.0));
    let et = drive(&mut eng, 0.0, 0.6, 0.1, 5);
    let snap = eng.snapshot();
    let mut alerts = AlertEngine::new(&AlertConfig::default()).expect("alerts");
    let mut raised = Vec::new();
    for i in 0..30 {
        let t = et + f64::from(i) * DT;
        let mut f = TelemetryFrame::new(i as u64, 0, Some(t));
        publish_energy(&mut f, Some(&snap));
        raised.extend(
            alerts
                .evaluate_frame(AlertClock::new(t, Some(t)), Some(5), &f)
                .into_iter()
                .filter(|n| n.state == AlertState::Raised),
        );
    }
    assert_eq!(raised.len(), 1, "{raised:?}");
    assert_eq!(raised[0].alert.rule_id, "energy_low");
}

/// Regression: a gradual refill (every per-sample step below `VE_REFILL_STEP_FRAC`) never cleared the
/// window, so the VE rate read negative during the refill and for a window afterwards.
#[test]
fn gradual_ve_refill_never_reports_a_negative_rate() {
    let mut eng = engine();
    let mut et = 0.0;
    let mut ve = 0.12;
    for _ in 0..20 {
        eng.update(&hybrid(et, ve, 0.5, 0.0, 0.0));
        et += DT;
        ve -= 0.001;
    }
    // 0.88 VE over 30 s at 100 Hz: about 0.0003 per sample.
    let steps = 3_000_u32;
    let start = ve;
    for i in 1..=steps {
        et += 0.01;
        ve = start + (1.0 - start) * f64::from(i) / f64::from(steps);
        eng.update(&hybrid(et, ve, 0.5, 0.0, 0.0));
        let rate = eng.snapshot().ve_rate_per_s.value;
        assert!(
            rate.is_none_or(|r| r >= 0.0),
            "negative rate {rate:?} at step {i}"
        );
    }
    // Stationary at the top: still no rate.
    eng.update(&hybrid(et + 0.01, 1.0, 0.5, 0.0, 0.0));
    assert!(!eng.snapshot().ve_rate_per_s.is_available());
    // Consumption resumes: a clean rate from the new window only.
    et += 0.01;
    for i in 1..=5 {
        eng.update(&hybrid(
            et + f64::from(i) * DT,
            1.0 - 0.001 * f64::from(i),
            0.5,
            0.0,
            0.0,
        ));
    }
    let rate = eng.snapshot().ve_rate_per_s.value.expect("rate");
    assert!((rate - 0.01).abs() < 1e-6, "{rate}");
}

/// Gate 1 pitstop1 return to the garage (real values): VE falls from 0.9392 to 0.2100 in 28 ms (ET
/// 315.8025 -> 315.8304) on the sample on which the lap counter went up, then holds at rest in the garage.
/// That is LMU resetting the allowance, not 73 % of use: the lap is incomplete for VE (no last-lap value,
/// no average), and the use-rate window never reads the reset as consumption.
#[test]
fn garage_return_ve_reset_is_not_use() {
    let mut eng = engine();
    let et = drive(&mut eng, 0.0, 1.0, 0.06, 2);
    assert!((eng.snapshot().ve_used_last_lap.value.unwrap_or(f64::NAN) - 0.06).abs() < 1e-9);
    let average_before = eng.snapshot().ve_per_lap.value;
    assert!(average_before.is_some());
    // Lap 3: driven down to 0.9392 (the real pre-return level), then the reset sample.
    let mut ve = 1.0 - 0.12;
    let mut t = et;
    for i in 0..SAMPLES_PER_LAP / 2 {
        t += DT;
        ve -= 0.0006;
        eng.update(&hybrid(
            t,
            ve,
            0.5,
            100.0,
            (i + 1) as f64 / SAMPLES_PER_LAP as f64,
        ));
    }
    eng.update(&hybrid(t + 0.0279, 0.21, 0.5, 0.0, 0.92));
    let rec = eng.lap_completed(true);
    assert_eq!(
        rec.ve_used, None,
        "a lap containing a VE reset has no VE use"
    );
    let s = eng.snapshot();
    assert_eq!(s.ve_used_last_lap.value, None);
    // Window average: still the two real laps only.
    assert_eq!(s.ve_per_lap.value, average_before);
    // At rest in the garage after the reset: no consumption rate from the reset.
    let mut g = t + 0.0279;
    for _ in 0..10 {
        g += 0.02;
        eng.update(&hybrid(g, 0.21, 0.5, 0.0, 0.92));
        let rate = eng.snapshot().ve_rate_per_s.value;
        assert!(
            rate.is_none_or(|r| r.abs() < 1e-9),
            "rate after the reset: {rate:?}"
        );
    }
    // The next full lap is measured normally from the reset level.
    for i in 0..SAMPLES_PER_LAP {
        g += DT;
        eng.update(&hybrid(
            g,
            0.21 - 0.0006 * (i + 1) as f64,
            0.5,
            100.0,
            (i + 1) as f64 / SAMPLES_PER_LAP as f64,
        ));
    }
    let rec = eng.lap_completed(true);
    assert!((rec.ve_used.unwrap_or(f64::NAN) - 0.06).abs() < 1e-9);
}

/// Normal use, however fast, is never a reset: the steepest real use per sample is far below
/// [`VE_RESET_STEP_FRAC`], so a drop just under it still counts as use.
#[test]
fn ve_drop_below_the_reset_step_is_use() {
    const DROP: f64 = 0.049;
    let mut eng = engine();
    eng.update(&hybrid(0.0, 0.8, 0.5, 100.0, 0.0));
    eng.update(&hybrid(0.1, 0.8 - DROP, 0.5, 100.0, 0.5));
    let rec = eng.lap_completed(true);
    assert!((rec.ve_used.unwrap_or(f64::NAN) - DROP).abs() < 1e-9);
}

/// A SoC jump (reset / refill) is neither deployment nor harvest: the lap is incomplete for SoC, VE stays
/// complete.
#[test]
fn soc_jump_makes_the_lap_incomplete_for_soc_only() {
    let mut eng = engine();
    eng.update(&hybrid(0.0, 0.8, 0.5, 100.0, 0.0));
    eng.update(&hybrid(0.1, 0.79, 0.49, 100.0, 0.3));
    eng.update(&hybrid(0.2, 0.78, 0.9, 100.0, 0.6));
    let rec = eng.lap_completed(true);
    assert_eq!(rec.soc_deployed, None);
    assert_eq!(rec.soc_harvested, None);
    assert!((rec.ve_used.unwrap_or(f64::NAN) - 0.02).abs() < 1e-9);
}

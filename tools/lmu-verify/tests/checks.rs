//! Gate-1 checks driven through PASS, FAIL and INCONCLUSIVE with synthetic captures.

mod common;

use common::{CHECK_IDS, Faults, Scenario, TeleportStepBack, assert_verdict, check, run};
use lmu_verify::analysis::checks::Verdict;
use lmu_verify::analysis::timing::NormalizerHandling;

#[test]
fn clean_run_passes_every_check() {
    let report = run(&Scenario::default());
    assert!(report.replay.complete, "{:?}", report.replay.stopped);
    let failures: Vec<String> = CHECK_IDS
        .iter()
        .map(|id| check(&report, id))
        .filter(|c| c.verdict != Verdict::Pass)
        .map(|c| {
            format!(
                "{} {:?}: {}\n  {}",
                c.id,
                c.verdict,
                c.summary,
                c.evidence.join("\n  ")
            )
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // Every published row a check lists was compared with its raw source.
    for (id, key) in [
        ("brake_temp_unit", "published_pairs"),
        ("tyre_side_mapping", "published_edge_pairs"),
    ] {
        let pairs = check(&report, id).metrics.get(key).copied().unwrap_or(0.0);
        assert!(pairs > 0.0, "{id}: {key} = {pairs}");
    }
    for id in ["virtual_energy", "state_of_charge"] {
        let c = check(&report, id);
        assert!(
            c.evidence
                .iter()
                .any(|l| l.starts_with("published energy.")),
            "{id}: {}",
            c.evidence.join("\n")
        );
        assert!(
            !c.docs_rows.contains(&"energy.ve_used_last_lap_frac"),
            "an uncompared derived row is listed"
        );
    }
}

#[test]
fn short_straight_run_is_inconclusive() {
    let report = run(&Scenario {
        garage_start: false,
        pit_lap: None,
        // 1.5 s at speed on the straight: too little of everything for any verdict.
        max_frames: 150,
        ..Scenario::default()
    });
    let decided: Vec<String> = CHECK_IDS
        .iter()
        .map(|id| check(&report, id))
        .filter(|c| c.verdict != Verdict::Inconclusive)
        .map(|c| format!("{} {:?}: {}", c.id, c.verdict, c.summary))
        .collect();
    assert!(decided.is_empty(), "{}", decided.join("\n"));
}

#[test]
fn unit_faults_fail_their_checks() {
    let report = run(&Scenario::with(Faults {
        brake_temp_celsius: true,
        tyre_temp_celsius: true,
        tyre_pressure_psi: true,
        brake_pressure_kpa: true,
        ve_percent: true,
        battery_percent: true,
        ..Faults::default()
    }));
    for id in [
        "brake_temp_unit",
        "tyre_temperature_unit",
        "tyre_pressure_unit",
        "brake_pressure_fraction",
        "virtual_energy",
        "state_of_charge",
    ] {
        assert_verdict(&report, id, Verdict::Fail);
    }
}

#[test]
fn sign_faults_fail_their_checks() {
    let report = run(&Scenario::with(Faults {
        invert_g_long: true,
        invert_steering: true,
        rotation_positive: true,
        tyre_mirror_right: true,
        wear_jitter: true,
        ..Faults::default()
    }));
    for id in [
        "g_long_sign",
        "steering_sign",
        "wheel_speed",
        "tyre_side_mapping",
        "tyre_wear_direction",
    ] {
        assert_verdict(&report, id, Verdict::Fail);
    }
    // Independent checks are unaffected.
    assert_verdict(&report, "g_lat_sign", Verdict::Pass);
    assert_verdict(&report, "brake_temp_unit", Verdict::Pass);
}

#[test]
fn inverted_lateral_g_fails() {
    let report = run(&Scenario::with(Faults {
        invert_g_lat: true,
        ..Faults::default()
    }));
    assert_verdict(&report, "g_lat_sign", Verdict::Fail);
    assert_verdict(&report, "steering_sign", Verdict::Pass);
}

#[test]
fn track_and_pit_faults_fail_their_checks() {
    let report = run(&Scenario::with(Faults {
        lap_dist_backwards: true,
        sector_direct: true,
        fuel_rises_on_track: true,
        in_pits_wrong: true,
        pit_state_unknown: true,
        ..Faults::default()
    }));
    for id in [
        "lap_distance",
        "sectors_and_laps",
        "fuel",
        "pit_flags",
        "pit_state_values",
    ] {
        assert_verdict(&report, id, Verdict::Fail);
    }
}

#[test]
fn et_regression_and_slow_clock_fail_the_timing_checks() {
    let report = run(&Scenario {
        laps: 1,
        pit_lap: None,
        faults: Faults {
            et_regression: true,
            slow_capture_clock: true,
            ..Faults::default()
        },
        ..Scenario::default()
    });
    assert_verdict(&report, "elapsed_time", Verdict::Fail);
    assert_verdict(&report, "update_rates", Verdict::Fail);
}

#[test]
fn frozen_channel_is_flagged() {
    let name = "powertrain.engine_rpm";
    let clean = run(&Scenario {
        laps: 1,
        pit_lap: None,
        ..Scenario::default()
    });
    let row = clean
        .channels
        .iter()
        .find(|r| r.name == name)
        .expect("rpm row");
    assert!(!row.frozen_while_moving, "{row:?}");
    let frozen = run(&Scenario {
        laps: 1,
        pit_lap: None,
        faults: Faults {
            frozen_rpm: true,
            ..Faults::default()
        },
        ..Scenario::default()
    });
    let row = frozen
        .channels
        .iter()
        .find(|r| r.name == name)
        .expect("rpm row");
    assert!(row.frozen_while_moving, "{row:?}");
    let report = lmu_verify::report::build(vec![frozen]);
    assert!(
        report
            .constant_findings
            .iter()
            .any(|f| f.name == name && f.value == Some(7_000.0))
    );
}

#[test]
fn capture_starting_just_before_the_line_does_not_judge_the_partial_lap() {
    // 50 m of lap 0 use 0.13 % VE: judged as a lap it would be "implausible" and fail A-LMU-013.
    let report = run(&Scenario {
        garage_start: false,
        start_m: Some(common::TRACK_M - 50.0),
        pit_lap: None,
        laps: 3,
        ..Scenario::default()
    });
    let c = check(&report, "virtual_energy");
    assert_eq!(
        c.verdict,
        Verdict::Pass,
        "{}\n{}",
        c.summary,
        c.evidence.join("\n")
    );
    assert_eq!(c.metrics.get("laps"), Some(&3.0), "{:?}", c.metrics);
    assert_eq!(c.metrics.get("partial_laps"), Some(&1.0), "{:?}", c.metrics);
    assert!(
        c.evidence
            .iter()
            .any(|l| l.contains("lap 0: 0.1") && l.contains("(partial)"))
    );
}

#[test]
fn one_complete_lap_is_not_enough_for_a_virtual_energy_pass() {
    // Garage out-lap (partial), lap 1 complete, then half of lap 2.
    let report = run(&Scenario {
        laps: 1,
        pit_lap: None,
        ..Scenario::default()
    });
    assert_verdict(&report, "virtual_energy", Verdict::Inconclusive);
}
#[test]
fn decisions_name_only_the_game_versions_of_the_deciding_captures() {
    let mut passing = run(&Scenario::default());
    let mut other = run(&Scenario {
        garage_start: false,
        pit_lap: None,
        max_frames: 150,
        ..Scenario::default()
    });
    let n = passing.replay.snapshots;
    // The passing capture is 1.4.2.0 apart from a few startup snapshots with gameVersion 0; the other one
    // (INCONCLUSIVE everywhere) is a newer build.
    passing.session.game_versions = [(0, 5), (14_200, n - 5)].into_iter().collect();
    other.session.game_versions = [(14_300, other.replay.snapshots)].into_iter().collect();
    let mixed_source = passing.clone();
    let report = lmu_verify::report::build(vec![passing, other]);
    let ve = report
        .decisions
        .iter()
        .find(|d| d.check == "virtual_energy")
        .expect("virtual_energy decision");
    assert_eq!(ve.verdict, Verdict::Pass);
    assert_eq!(ve.game_versions, [14_200]);
    assert!(
        ve.action
            .starts_with("can move to VERIFIED for gameVersion 14200 (1 of 2 capture(s) passed)"),
        "{}",
        ve.action
    );
    // A capture without one dominant known version cannot make anything VERIFIED.
    let mut mixed = mixed_source;
    mixed.session.game_versions = [(14_200, 10), (14_300, 10)].into_iter().collect();
    let report = lmu_verify::report::build(vec![mixed]);
    let ve = report
        .decisions
        .iter()
        .find(|d| d.check == "virtual_energy")
        .expect("virtual_energy decision");
    assert!(ve.game_versions.is_empty());
    assert!(
        ve.action.contains("cannot move to VERIFIED"),
        "{}",
        ve.action
    );
}

#[test]
fn session_end_rewind_is_explained_not_a_fault() {
    // stint1 (A-LMU-031): after the session is over and the player is back in the garage, LMU rewinds ET to
    // just after the session end. Explained, so the elapsed-time check does not fail on it.
    let report = run(&Scenario {
        laps: 1,
        pit_lap: None,
        faults: Faults {
            session_end_rewind: true,
            ..Faults::default()
        },
        ..Scenario::default()
    });
    let timing = report.timing.as_ref().expect("timing");
    assert_eq!(timing.et_regressions, 1);
    assert_eq!(timing.unexplained_et_regressions, 0);
    assert!(timing.regressions[0].session_end_rewind);
    assert!(
        timing.regressions[0]
            .explanation
            .as_deref()
            .is_some_and(|e| e.contains("session-end rewind"))
    );
    assert_eq!(timing.scoring_et_regressions_not_restart, 0);
    assert_eq!(timing.scoring_et_session_end_rewinds, 1);
    assert_eq!(report.normalizer.session_end_rewinds, 1);
    assert_ne!(check(&report, "elapsed_time").verdict, Verdict::Fail);
}

#[test]
fn uninitialised_soc_mitigation_is_not_a_contradiction() {
    // gate1-hypercar (A-LMU-014): mSoC reads 0 while the battery fraction is charged; the normalizer publishes
    // the battery fraction as the state of charge. The check must expect that, not mSoC / 100.
    let report = run(&Scenario::with(Faults {
        soc_uninitialised: true,
        ..Faults::default()
    }));
    let soc = check(&report, "state_of_charge");
    assert_eq!(soc.verdict, Verdict::Pass, "{}", soc.summary);
    assert!(
        soc.metrics
            .get("soc_substituted_percent")
            .is_some_and(|n| *n > 0.0),
        "{:?}",
        soc.metrics
    );
}

#[test]
fn rising_tyre_wear_contradicts_the_documented_direction() {
    // A-LMU-040: 1.0 = new, falling while driving. A build where wear rises must FAIL, not PASS.
    let report = run(&Scenario::with(Faults {
        wear_rising: true,
        ..Faults::default()
    }));
    let wear = check(&report, "tyre_wear_direction");
    assert_eq!(wear.verdict, Verdict::Fail, "{}", wear.summary);
    assert!(
        wear.summary.contains("contradicts A-LMU-040"),
        "{}",
        wear.summary
    );
}

/// The elapsed-time verdict for a garage teleport with an ET step-back follows what the normalizer did.
fn garage_teleport(kind: TeleportStepBack) -> lmu_verify::analysis::CaptureReport {
    run(&Scenario {
        laps: 1,
        pit_lap: None,
        garage_s: 6.0,
        faults: Faults {
            garage_teleport: Some(kind),
            ..Faults::default()
        },
        ..Scenario::default()
    })
}

#[test]
fn teleport_step_back_dropped_by_the_normalizer_is_explained() {
    let report = garage_teleport(TeleportStepBack::SameSample);
    let timing = report.timing.as_ref().expect("timing");
    assert_eq!(timing.et_regressions, 1);
    assert_eq!(report.normalizer.teleport_et_step_backs, 1);
    let r = &timing.regressions[0];
    assert_eq!(r.normalizer, NormalizerHandling::TeleportStepBackDropped);
    assert!(r.teleport_step_back_geometry);
    assert_eq!(timing.unexplained_et_regressions, 0, "{}", r.describe());
    assert_ne!(check(&report, "elapsed_time").verdict, Verdict::Fail);
}

#[test]
fn teleport_then_step_back_is_unexplained_while_the_normalizer_restarts() {
    // pitstop1 (second review): the teleport lands on an advancing sample, the 10 ms step-back on the next one
    // with no position jump. While the normalizer turns that into a session restart (a false SessionChanged),
    // the step-back is UNEXPLAINED and elapsed_time FAILs; once the normalizer drops it, it is explained.
    let report = garage_teleport(TeleportStepBack::NextSample);
    let timing = report.timing.as_ref().expect("timing");
    assert_eq!(timing.et_regressions, 1);
    let r = &timing.regressions[0];
    assert!(!r.teleport_step_back_geometry);
    match r.normalizer {
        NormalizerHandling::Restart => {
            assert_eq!(timing.unexplained_et_regressions, 1);
            assert!(r.describe().starts_with("UNEXPLAINED"), "{}", r.describe());
            assert_eq!(check(&report, "elapsed_time").verdict, Verdict::Fail);
        }
        NormalizerHandling::TeleportStepBackDropped => {
            assert_eq!(timing.unexplained_et_regressions, 0, "{}", r.describe());
        }
        other => panic!("unexpected normalizer handling {other:?}"),
    }
}

/// Published rows a check judges directly (its verdict reads the published channel itself, not a raw field).
const JUDGED_DIRECTLY: [(&str, &str); 6] = [
    ("g_long_sign", "dynamics.g_long"),
    ("g_lat_sign", "dynamics.g_lat"),
    ("g_lat_sign", "dynamics.yaw_rate_radps"),
    ("g_lat_sign", "tyres.{w}.load_n"),
    ("steering_sign", "inputs.steering_raw"),
    ("wheel_speed", "wheels.{w}.speed_mps"),
];

#[test]
fn every_listed_published_row_is_compared_or_judged() {
    // Second review F3: a check recommends a channel-map row for VERIFIED only when it has evidence on the
    // published channel, either by judging it directly or by comparing it with its raw source.
    let report = run(&Scenario::default());
    let mut missing = Vec::new();
    for c in &report.checks {
        for row in &c.docs_rows {
            let judged = JUDGED_DIRECTLY.contains(&(c.id, *row));
            // Older evidence lines name the row by its channel suffix (`published brakes.*.temp_c`).
            let suffix = row.rsplit('.').next().unwrap_or(row);
            let compared = c
                .evidence
                .iter()
                .any(|l| l.contains("published") && (l.contains(row) || l.contains(suffix)));
            if !judged && !compared {
                missing.push(format!("{}: {row}", c.id));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "rows listed but never compared: {missing:?}"
    );
}

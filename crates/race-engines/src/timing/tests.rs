//! Timing engine tests. Synthetic laps are generated in-test (no fabricated data leaves the tests).

use telemetry_model::{TelemetryFrame, Value, ch};

use super::*;

const L: f64 = 6_000.0;
const STEP_M: f64 = 0.5;
const T0: f64 = 1_000.0;
const EPS: f64 = 1e-6;

fn key(session: u64) -> CompatibilityKey {
    CompatibilityKey {
        track_id: Some("TEST_6000".into()),
        vehicle: Some("Test Car".into()),
        class: Some("Hypercar".into()),
        session,
    }
}

/// Piecewise constant-speed lap: `(segment end distance, segment duration)`.
#[derive(Clone)]
struct Profile(Vec<(f64, f64)>);

impl Profile {
    fn uniform(lap_time: f64) -> Self {
        Self(vec![(L, lap_time)])
    }

    fn time_at(&self, d: f64) -> f64 {
        let mut start_d = 0.0;
        let mut t = 0.0;
        for &(end, dur) in &self.0 {
            if d <= end {
                return t + dur * (d - start_d) / (end - start_d);
            }
            t += dur;
            start_d = end;
        }
        t
    }

    fn lap_time(&self) -> f64 {
        self.0.iter().map(|s| s.1).sum()
    }
}

fn three_sectors(d: f64) -> u8 {
    if d < 2_000.0 {
        0
    } else if d < 4_000.0 {
        1
    } else {
        2
    }
}

/// Drives an engine with synthetic laps. Each lap starts with a sample exactly on the line.
struct Sim {
    eng: TimingEngine,
    lap: i32,
    lap_start: f64,
    line_et: f64,
    with_lap_start: bool,
    official: Option<f64>,
}

impl Sim {
    fn new(config: TimingConfig) -> Self {
        let mut eng = TimingEngine::new(config).expect("valid config");
        eng.set_context(key(1));
        let mut sim = Self {
            eng,
            lap: 0,
            lap_start: T0 - 150.0,
            line_et: T0,
            with_lap_start: true,
            official: None,
        };
        // Pre-roll: the end of lap 0, so the first line crossing is observed.
        sim.raw(L - 0.6, T0 - 0.01, false, false);
        sim
    }

    fn sample(&self, d: f64, et: f64, in_pit: bool, invalid: bool) -> TimingSample {
        TimingSample {
            et_s: et,
            lap_number: Some(self.lap),
            lap_start_et_s: self.with_lap_start.then_some(self.lap_start),
            lap_dist_m: Some(d),
            scoring_lap_dist_m: None,
            track_length_m: Some(L),
            sector_index: Some(three_sectors(d)),
            lap_invalidated: Some(invalid),
            in_pit_lane: Some(in_pit),
            official_last_lap_s: self.official,
            discontinuity: None,
        }
    }

    fn raw(&mut self, d: f64, et: f64, in_pit: bool, invalid: bool) -> TimingUpdate {
        let s = self.sample(d, et, in_pit, invalid);
        self.eng.update(&s)
    }

    /// Crosses the line and drives one full lap (the lap completes on the next lap's first sample).
    fn drive(
        &mut self,
        p: &Profile,
        flags: impl Fn(f64) -> (bool, bool),
        mut probe: impl FnMut(f64, &TimingEngine, &TimingUpdate),
    ) -> Vec<TimingUpdate> {
        self.lap += 1;
        self.lap_start = self.line_et;
        let n = (L / STEP_M) as usize;
        let mut ups = Vec::with_capacity(n);
        for k in 0..n {
            let d = k as f64 * STEP_M;
            let et = self.line_et + p.time_at(d);
            let (pit, inv) = flags(d);
            let u = self.raw(d, et, pit, inv);
            probe(d, &self.eng, &u);
            ups.push(u);
        }
        self.line_et += p.lap_time();
        ups
    }

    fn lap_plain(&mut self, p: &Profile) -> Vec<TimingUpdate> {
        self.drive(p, |_| (false, false), |_, _, _| {})
    }

    /// Crosses the line once more so the last driven lap completes.
    fn cross(&mut self) -> TimingUpdate {
        self.lap += 1;
        self.lap_start = self.line_et;
        let et = self.line_et;
        self.raw(0.0, et, false, false)
    }
}

fn completed(ups: &[TimingUpdate]) -> Vec<LapRecord> {
    ups.iter().filter_map(|u| u.completed_lap.clone()).collect()
}

fn lap_b() -> Profile {
    // Known offsets to the uniform 100 s lap A: +0.5 at S1 (2000 m), +0.25 at the middle (3000 m),
    // -0.5 at S2 (4000 m), -0.3 at the finish.
    let third = 100.0 / 3.0;
    let sixth = 100.0 / 6.0;
    Profile(vec![
        (2_000.0, third + 0.5),
        (3_000.0, sixth - 0.25),
        (4_000.0, sixth - 0.75),
        (L, third + 0.2),
    ])
}

#[test]
fn delta_is_distance_aligned_at_start_s1_middle_s2_and_finish() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0));
    let mut seen = Vec::new();
    let ups = sim.drive(
        &lap_b(),
        |_| (false, false),
        |d, eng, _| {
            if [0.0, 2_000.0, 3_000.0, 4_000.0, L - STEP_M].contains(&d) {
                let snap = eng.snapshot();
                seen.push((
                    d,
                    snap.delta_to_personal_best.value,
                    snap.delta_to_reference.value,
                    snap.predicted_lap.value,
                ));
            }
        },
    );
    let lap_a = completed(&ups);
    assert_eq!(lap_a.len(), 1, "lap A completes on lap B's first sample");
    assert!((lap_a[0].time_s.expect("time") - 100.0).abs() < EPS);
    assert!(lap_a[0].trace_complete);

    let expected = [
        (0.0, 0.0),
        (2_000.0, 0.5),
        (3_000.0, 0.25),
        (4_000.0, -0.5),
        // Last sample before the line, inside the -0.5 → -0.3 segment.
        (L - STEP_M, -0.5 + 0.2 * (L - STEP_M - 4_000.0) / 2_000.0),
    ];
    assert_eq!(seen.len(), expected.len());
    for ((d, delta_pb, delta_ref, predicted), (ed, exp)) in seen.iter().zip(expected) {
        assert_eq!(*d, ed);
        let dp = delta_pb.expect("delta to PB available");
        assert!((dp - exp).abs() < 1e-3, "delta at {d}: {dp} vs {exp}");
        // Default reference is the personal best.
        assert_eq!(*delta_ref, *delta_pb);
        let pr = predicted.expect("predicted lap");
        assert!((pr - (100.0 + exp)).abs() < 1e-3);
    }

    // Finish: the completed lap B is 0.3 s faster than lap A.
    let up = sim.cross();
    let b = up.completed_lap.expect("lap B completes");
    assert!((b.time_s.expect("time") - 99.7).abs() < EPS);
    assert_eq!(b.time_source, LapTimeSource::LmuLapStart);
    let snap = sim.eng.snapshot();
    assert_eq!(snap.personal_best_lap, Some(b.id));
    assert!((snap.personal_best.value.expect("pb") - 99.7).abs() < EPS);
}

#[test]
fn sectors_bests_and_theoretical_best_with_source_index() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0));
    sim.lap_plain(&lap_b());
    sim.cross();
    let snap = sim.eng.snapshot();
    assert_eq!(snap.sector_count, 3);
    let laps: Vec<_> = sim.eng.laps().cloned().collect();
    // Lap 0 (joined) + A + B.
    assert_eq!(laps.len(), 3);
    let a = &laps[1];
    let b = &laps[2];
    // Index transitions are timed at the midpoint between samples: half a sample of uncertainty.
    let tol = 0.5 * STEP_M / 60.0 + EPS;
    let third = 100.0 / 3.0;
    for s in &a.sectors {
        assert!((s.expect("sector") - third).abs() < tol);
    }
    let b_s = [third + 0.5, 100.0 / 3.0 - 1.0, third + 0.2];
    for (s, e) in b.sectors.iter().zip(b_s) {
        assert!((s.expect("sector") - e).abs() < 0.02, "{s:?} vs {e}");
    }
    let tb = snap.theoretical_best.value.expect("theoretical best");
    let expected = third + (third - 1.0) + third;
    assert!((tb - expected).abs() < 0.02, "{tb} vs {expected}");
    assert_eq!(snap.theoretical_best.formula, formula::THEORETICAL_BEST);
}

#[test]
fn dynamic_distance_sectors_and_compatibility() {
    let config = TimingConfig {
        sector_model: SectorModel::DistanceBoundaries(vec![1_200.0, 2_400.0, 3_600.0, 4_800.0]),
        ..TimingConfig::default()
    };
    let mut sim = Sim::new(config);
    sim.lap_plain(&Profile::uniform(100.0));
    let b = Profile(vec![
        (1_200.0, 19.5),
        (2_400.0, 20.5),
        (3_600.0, 20.0),
        (4_800.0, 19.0),
        (L, 21.5),
    ]);
    sim.lap_plain(&b);
    sim.cross();
    let snap = sim.eng.snapshot();
    assert_eq!(snap.sector_count, 5);
    let laps: Vec<_> = sim.eng.laps().cloned().collect();
    for s in &laps[1].sectors {
        assert!((s.expect("sector") - 20.0).abs() < 1e-9);
    }
    let tb = snap.theoretical_best.value.expect("tb");
    assert!((tb - 98.5).abs() < 1e-9, "{tb}");

    // Another session: nothing is comparable.
    sim.eng.set_context(key(2));
    let snap = sim.eng.snapshot();
    assert_eq!(snap.personal_best.value, None);
    assert_eq!(snap.theoretical_best.value, None);
    assert!(snap.best_sectors.iter().all(|m| m.value.is_none()));
}

#[test]
fn cross_session_only_when_explicitly_allowed() {
    let config = TimingConfig {
        allow_cross_session_references: true,
        ..TimingConfig::default()
    };
    let mut sim = Sim::new(config);
    sim.lap_plain(&Profile::uniform(100.0));
    sim.cross();
    sim.eng.set_context(key(2));
    assert!(sim.eng.snapshot().personal_best.value.is_some());
    let mut other_car = key(2);
    other_car.vehicle = Some("Other".into());
    sim.eng.set_context(other_car);
    assert!(sim.eng.snapshot().personal_best.value.is_none());
}

#[test]
fn counter_first_then_late_wrap_counts_once_and_confirms() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.with_lap_start = false;
    sim.lap_plain(&Profile::uniform(100.0));
    // Line crossing: counter moves, but the (scoring-derived) distance still reads the end of the lap.
    sim.lap += 1;
    let t = sim.line_et;
    let u1 = sim.raw(L - 0.3, t, false, false);
    let u2 = sim.raw(L - 0.1, t + 0.01, false, false);
    let u3 = sim.raw(1.0, t + 0.02, false, false);
    let u4 = sim.raw(1.6, t + 0.03, false, false);
    let all = [u1, u2, u3, u4];
    let done = completed(&all);
    assert_eq!(done.len(), 1, "exactly one lap completed");
    let id = done[0].id;
    assert_eq!(
        done[0].flags.end_evidence.count(),
        1,
        "counter only at first"
    );
    // The late wrap upgraded the evidence.
    let lap = sim.eng.lap(id).expect("lap in history");
    assert!(lap.flags.end_evidence.distance_wrap);
    assert!(lap.time_trusted());
    assert_eq!(sim.eng.snapshot().laps_completed, 2);
}

#[test]
fn wrap_first_then_counter_counts_once() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.with_lap_start = false;
    sim.lap_plain(&Profile::uniform(100.0));
    let t = sim.line_et;
    // Distance wraps before the counter moves.
    // The lap's last sample is at t - 0.0083 s.
    let u1 = sim.raw(1.0, t - 0.006, false, false);
    let u2 = sim.raw(1.5, t - 0.003, false, false);
    assert!(u1.completed_lap.is_none() && u2.completed_lap.is_none());
    sim.lap += 1;
    let u3 = sim.raw(2.0, t, false, false);
    let lap = u3.completed_lap.expect("counter completes the lap");
    assert!(lap.flags.end_evidence.lap_counter && lap.flags.end_evidence.distance_wrap);
    assert!(lap.time_trusted());
    let u4 = sim.raw(2.5, t + 0.01, false, false);
    assert!(u4.completed_lap.is_none());
}

#[test]
fn counter_alone_is_not_trusted_and_reversing_over_the_line_is_not_a_lap() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.with_lap_start = false;
    sim.lap_plain(&Profile::uniform(100.0));
    let t = sim.line_et;
    // Back over the line and forward again without the counter moving: no lap.
    let u1 = sim.raw(5.0, t, false, false);
    let u2 = sim.raw(L - 5.0, t + 1.0, false, false);
    let u3 = sim.raw(5.0, t + 2.0, false, false);
    assert!(completed(&[u1, u2, u3]).is_empty());
    // Counter moves far from any wrap: the lap completes but is not trusted.
    sim.lap += 1;
    let u4 = sim.raw(10.0, t + 10.0, false, false);
    let lap = u4.completed_lap.expect("counter is primary");
    assert!(!lap.time_trusted());
    assert!(!lap.eligible_for_best());
}

#[test]
fn counter_jump_and_et_regression_abandon_the_lap() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0));
    let before = sim.eng.snapshot().laps_completed;
    sim.lap += 2;
    let t = sim.line_et;
    let u = sim.raw(10.0, t, false, false);
    assert!(u.completed_lap.is_none());
    assert_eq!(u.reset, Some(ResetReason::LapCounterJump));
    assert!(u.abandoned_lap.is_some());
    assert_eq!(sim.eng.snapshot().laps_completed, before);

    let u = sim.raw(20.0, t - 50.0, false, false);
    assert_eq!(u.reset, Some(ResetReason::EtRegression));
    assert!(u.abandoned_lap.is_some());
    // Duplicate time: ignored.
    let u = sim.raw(20.0, t - 50.0, false, false);
    assert_eq!(u, TimingUpdate::default());
}

#[test]
fn pit_and_invalid_laps_are_excluded_from_bests() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0)); // lap 1: clean
    sim.drive(
        &Profile::uniform(95.0),
        |d| (false, d > 3_000.0),
        |_, _, _| {},
    ); // lap 2: invalidated
    sim.drive(
        &Profile::uniform(96.0),
        |d| (d > 5_800.0, false),
        |_, _, _| {},
    ); // lap 3: in-lap
    sim.drive(
        &Profile::uniform(97.0),
        |d| (d < 200.0, false),
        |_, _, _| {},
    ); // lap 4: out-lap
    sim.lap_plain(&Profile::uniform(99.0)); // lap 5: clean
    sim.cross();
    let laps: Vec<_> = sim.eng.laps().cloned().collect();
    assert!(laps[2].flags.invalidated && !laps[2].eligible_for_best());
    assert!(laps[3].flags.pit_in && !laps[3].eligible_for_best());
    assert!(laps[4].flags.pit_out && !laps[4].eligible_for_best());
    let snap = sim.eng.snapshot();
    assert_eq!(snap.personal_best_lap, Some(laps[5].id));
    assert!((snap.personal_best.value.expect("pb") - 99.0).abs() < EPS);
}

#[test]
fn previous_and_selected_references() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0));
    sim.lap_plain(&Profile::uniform(102.0));
    let laps: Vec<_> = sim.eng.laps().cloned().collect();
    let lap_a = laps[1].id;
    sim.eng
        .select_reference(ReferenceKind::PreviousLap)
        .expect("previous lap");
    let mut probe = Vec::new();
    sim.drive(
        &Profile::uniform(101.0),
        |_| (false, false),
        |d, eng, _| {
            if d == 3_000.0 {
                let s = eng.snapshot();
                probe.push((
                    s.delta_to_reference.value,
                    s.reference_lap,
                    s.delta_to_personal_best.value,
                ));
            }
        },
    );
    let (d_ref, ref_lap, d_pb) = probe[0];
    // Middle of the lap: 50.5 s now vs 51 s on the previous (102 s) lap and 50 s on the PB.
    assert!((d_ref.expect("delta prev") + 0.5).abs() < 1e-6);
    assert_ne!(ref_lap, Some(lap_a));
    assert!((d_pb.expect("delta pb") - 0.5).abs() < 1e-6);

    sim.eng
        .select_reference(ReferenceKind::SelectedLap(lap_a))
        .expect("selected lap");
    assert_eq!(sim.eng.snapshot().reference_lap, Some(lap_a));
    assert_eq!(
        sim.eng
            .select_reference(ReferenceKind::SelectedLap(LapId(9_999))),
        Err(TimingError::UnknownLap(LapId(9_999)))
    );
    // The joined first lap has no trace.
    let joined = laps[0].id;
    assert_eq!(
        sim.eng.select_reference(ReferenceKind::SelectedLap(joined)),
        Err(TimingError::NoTrace(joined))
    );
}

#[test]
fn session_best_of_another_car_has_no_distance_delta() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0));
    sim.eng.set_external_session_best(Some(95.0));
    sim.eng
        .select_reference(ReferenceKind::SessionBest)
        .expect("session best");
    sim.lap_plain(&Profile::uniform(100.0));
    let s = sim.eng.snapshot();
    assert_eq!(s.session_best.value, Some(95.0));
    assert_eq!(s.session_best_holder, Some(SessionBestHolder::OtherCar));
    assert_eq!(s.reference_status, ReferenceStatus::OtherCar);
    assert_eq!(s.delta_to_session_best.value, None);
    assert_eq!(s.delta_to_reference.value, None);
    assert!(s.delta_to_personal_best.value.is_some());
}

#[test]
fn official_lap_time_adopted_only_within_tolerance() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0));
    sim.official = Some(100.004);
    sim.lap_plain(&Profile::uniform(99.7));
    sim.official = Some(98.0);
    sim.cross();
    let laps: Vec<_> = sim.eng.laps().cloned().collect();
    assert_eq!(laps[1].time_source, LapTimeSource::Official);
    assert_eq!(laps[1].time_s, Some(100.004));
    assert_eq!(
        laps[1].time_metric().provenance,
        telemetry_model::Provenance::Measured
    );
    let trace = sim.eng.trace(laps[1].id).expect("trace kept");
    assert!(trace.validate().is_ok());
    assert_eq!(trace.lap_time_s, 100.004);
    assert_eq!(laps[2].time_source, LapTimeSource::LmuLapStart);
    assert_eq!(laps[2].official_mismatch_s, Some(98.0));
    assert!((laps[2].time_s.expect("time") - 99.7).abs() < EPS);
}

#[test]
fn joined_mid_lap_lap_is_timed_by_lmu_but_not_a_reference() {
    let mut eng = TimingEngine::new(TimingConfig::default()).expect("engine");
    eng.set_context(key(1));
    let lap_start = 500.0;
    for k in 0..=6_000 {
        let d = 3_000.0 + k as f64 * 0.5;
        if d >= L {
            break;
        }
        let s = TimingSample {
            et_s: lap_start + d / 60.0,
            lap_number: Some(4),
            lap_start_et_s: Some(lap_start),
            lap_dist_m: Some(d),
            track_length_m: Some(L),
            ..TimingSample::default()
        };
        eng.update(&s);
    }
    let s = TimingSample {
        et_s: lap_start + 100.0,
        lap_number: Some(5),
        lap_start_et_s: Some(lap_start + 100.0),
        lap_dist_m: Some(0.0),
        track_length_m: Some(L),
        ..TimingSample::default()
    };
    let lap = eng.update(&s).completed_lap.expect("lap completes");
    assert!(lap.flags.joined_mid_lap);
    assert!(lap.time_trusted());
    assert!(!lap.trace_complete);
    assert!((lap.time_s.expect("time") - 100.0).abs() < EPS);
    let snap = eng.snapshot();
    assert_eq!(snap.personal_best.value, lap.time_s);
    assert_eq!(snap.reference_status, ReferenceStatus::NoTrace);
    assert_eq!(snap.delta_to_personal_best.value, None);
}

#[test]
fn imported_lap_can_be_selected_only_in_a_compatible_context() {
    let mut eng = TimingEngine::new(TimingConfig::default()).expect("engine");
    eng.set_context(key(1));
    let points: Vec<TracePoint> = (0..=60)
        .map(|i| TracePoint {
            dist_m: i as f64 * 100.0,
            elapsed_s: i as f64 * 100.0 / 60.0,
        })
        .collect();
    let id = eng
        .import_lap(key(1), Some(3), vec![], 100.0, points.clone())
        .expect("valid import");
    eng.select_reference(ReferenceKind::SelectedLap(id))
        .expect("compatible");
    let other = eng
        .import_lap(key(7), None, vec![], 100.0, points.clone())
        .expect("valid import");
    assert_eq!(
        eng.select_reference(ReferenceKind::SelectedLap(other)),
        Err(TimingError::IncompatibleLap(other))
    );
    let mut bad = points;
    bad[0].dist_m = 1.0;
    assert!(matches!(
        eng.import_lap(key(1), None, vec![], 100.0, bad),
        Err(TimingError::InvalidTrace(_))
    ));
}

#[test]
fn trace_interpolation_and_bounds() {
    let t = LapTrace {
        lap_id: LapId(0),
        key: key(1),
        lap_time_s: 10.0,
        points: (0..=10)
            .map(|i| TracePoint {
                dist_m: i as f64 * 10.0,
                elapsed_s: i as f64,
            })
            .collect(),
    };
    assert!(t.validate().is_ok());
    assert_eq!(t.elapsed_at(0.0), Some(0.0));
    assert_eq!(t.elapsed_at(55.0), Some(5.5));
    assert_eq!(t.elapsed_at(100.0), Some(10.0));
    assert_eq!(t.elapsed_at(100.1), None);
    assert_eq!(t.elapsed_at(-0.1), None);
    assert_eq!(t.elapsed_at(f64::NAN), None);
    assert_eq!(t.length_m(), 100.0);
}

#[test]
fn invalid_configuration_is_rejected() {
    let bad = |b: Vec<f64>| {
        TimingEngine::new(TimingConfig {
            sector_model: SectorModel::DistanceBoundaries(b),
            ..TimingConfig::default()
        })
        .err()
    };
    assert!(matches!(
        bad(vec![100.0, 50.0]),
        Some(TimingError::InvalidSectorBoundaries { .. })
    ));
    assert!(matches!(
        bad(vec![0.0]),
        Some(TimingError::InvalidSectorBoundaries { .. })
    ));
    assert!(matches!(
        bad((1..=MAX_SECTORS).map(|i| i as f64).collect()),
        Some(TimingError::TooManySectors { .. })
    ));
    assert!(matches!(
        TimingEngine::new(TimingConfig {
            trace_resolution_m: 0.0,
            ..TimingConfig::default()
        }),
        Err(TimingError::InvalidTraceResolution(_))
    ));
}

#[test]
fn sample_from_frame_prefers_progress_distance() {
    let mut f = TelemetryFrame::new(1, 0, Some(12.5));
    f.set_valid(ch::LAP_NUMBER, Value::I32(3)).expect("set");
    f.set_valid(ch::LAP_START_TIME, Value::F64(10.0))
        .expect("set");
    f.set_valid(ch::LAP_PROGRESS, Value::F32(0.25))
        .expect("set");
    f.set_valid(ch::TRACK_LENGTH, Value::F64(4_000.0))
        .expect("set");
    f.set_valid(ch::LAP_DISTANCE, Value::F64(990.0))
        .expect("set");
    f.set_valid(ch::SECTOR, Value::U8(1)).expect("set");
    f.set_valid(ch::IN_PIT_LANE, Value::Bool(true))
        .expect("set");
    f.set_valid(ch::LAST_LAP_TIME, Value::F64(95.0))
        .expect("set");
    let s = TimingSample::from_frame(&f).expect("sample");
    assert_eq!(s.et_s, 12.5);
    assert_eq!(s.lap_number, Some(3));
    assert_eq!(s.lap_dist_m, Some(1_000.0));
    assert_eq!(s.sector_index, Some(1));
    assert_eq!(s.in_pit_lane, Some(true));
    // Unavailable channels stay None, never 0.
    assert_eq!(s.lap_invalidated, None);
    assert_eq!(s.official_last_lap_s, Some(95.0));
    let empty = TelemetryFrame::new(1, 0, None);
    assert!(TimingSample::from_frame(&empty).is_none());
}

/// Real LMU behaviour (A-LMU-070, A-LMU-031): the lap distance is negative on the grid, LMU re-bases it by
/// one lap at the race start without a lap increment, and after the session end it rewinds the elapsed time.
/// Neither may count a lap or produce a negative time.
#[test]
fn grid_rebase_and_session_end_rewind_count_no_fake_lap() {
    let mut eng = TimingEngine::new(TimingConfig::default()).expect("engine");
    eng.set_context(key(1));
    let s = |et: f64, lap: i32, start: f64, d: f64| TimingSample {
        lap_number: Some(lap),
        lap_start_et_s: Some(start),
        lap_dist_m: Some(d),
        track_length_m: Some(L),
        sector_index: Some(three_sectors(d.max(0.0))),
        lap_invalidated: Some(false),
        in_pit_lane: Some(false),
        ..TimingSample::new(et)
    };
    let mut completed = Vec::new();
    // Grid: 150 m behind the line, negative distance, stationary.
    for i in 0..50 {
        let up = eng.update(&s(50.0 + 0.1 * f64::from(i), 0, 0.0, -150.0));
        completed.extend(up.completed_lap);
    }
    // Race start: the distance is re-based to L - 150 without a lap increment, then the car rolls to the
    // line at 30 m/s (5 s).
    for i in 0..50 {
        let d = L - 150.0 + 3.0 * f64::from(i);
        let up = eng.update(&s(60.0 + 0.1 * f64::from(i), 0, 0.0, d));
        completed.extend(up.completed_lap);
    }
    assert!(
        completed.is_empty(),
        "the re-base is not a lap: {completed:?}"
    );
    // First crossing at 65.0 s: counter, lap start and distance wrap together.
    let line = 65.0;
    let lap_time = 100.0;
    let mut et = line;
    while et < line + lap_time {
        let d = L * (et - line) / lap_time;
        let up = eng.update(&s(et, 1, line, d));
        completed.extend(up.completed_lap);
        et += 0.1;
    }
    // Second crossing.
    let line2 = line + lap_time;
    for i in 0..300 {
        let et = line2 + 0.1 * f64::from(i);
        let d = L * (et - line2) / lap_time;
        let up = eng.update(&s(et, 2, line2, d));
        completed.extend(up.completed_lap);
    }
    // The grid lap 0 started behind the line: never a best or a reference. Lap 1 is a full trusted lap.
    assert_eq!(completed.len(), 2, "{completed:?}");
    assert!(completed[0].flags.before_line);
    assert!(!completed[0].eligible_for_best());
    assert!(!completed[1].flags.before_line);
    let lap1 = &completed[1];
    assert_eq!(lap1.lap_number, Some(1));
    assert!(lap1.time_trusted());
    assert!((lap1.time_s.expect("time") - lap_time).abs() < 0.05);

    // Session end: ET rewinds by 42.85 s while the lap start stays after it.
    let rewind_et = line2 + 30.0 - 42.85;
    let up = eng.update(&s(rewind_et, 2, line2, 1_800.0));
    assert!(up.completed_lap.is_none());
    assert_eq!(up.reset, Some(ResetReason::EtRegression));
    for i in 1..100 {
        let up = eng.update(&s(
            rewind_et + 0.1 * f64::from(i),
            2,
            line2,
            1_800.0 + f64::from(i),
        ));
        assert!(up.completed_lap.is_none(), "fake lap after the rewind");
        let t = eng.snapshot().current_lap_time.value;
        assert!(t.is_none_or(|t| t >= 0.0), "negative lap time {t:?}");
    }
}

#[test]
fn repeated_official_lap_time_is_adopted_but_stale_value_is_not() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0)); // lap A
    sim.official = Some(100.0);
    sim.lap_plain(&Profile::uniform(100.0)); // A completes with a new official value; lap B
    sim.lap_plain(&Profile::uniform(99.9)); // B completes, official unchanged (same time); lap C
    sim.lap_plain(&Profile::uniform(100.0)); // C completes, official unchanged (stale for C)
    let laps: Vec<_> = sim.eng.laps().cloned().collect();
    let [a, b, c] = &laps[laps.len() - 3..] else {
        panic!("three laps expected: {laps:?}");
    };
    assert_eq!(a.time_source, LapTimeSource::Official);
    assert_eq!(b.time_source, LapTimeSource::Official, "repeat adopted");
    assert_eq!(b.time_s, Some(100.0));
    assert_ne!(
        c.time_source,
        LapTimeSource::Official,
        "stale value kept out"
    );
    assert_eq!(c.official_mismatch_s, None);
    assert!((c.time_s.expect("time") - 99.9).abs() < EPS);
}

#[test]
fn unchanged_official_value_is_not_a_repeat_without_a_per_lap_update() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0)); // lap A
    sim.official = Some(100.0);
    sim.lap_plain(&Profile::uniform(99.9)); // A completes with a new official value; lap B
    sim.lap_plain(&Profile::uniform(100.0)); // B: value unchanged and 0.1 s off -> stale, not adopted
    sim.lap_plain(&Profile::uniform(100.0)); // C: unchanged, matches, but LMU skipped B's update
    let laps: Vec<_> = sim.eng.laps().cloned().collect();
    let [a, b, c] = &laps[laps.len() - 3..] else {
        panic!("three laps expected: {laps:?}");
    };
    assert_eq!(a.time_source, LapTimeSource::Official);
    assert_ne!(b.time_source, LapTimeSource::Official);
    assert_ne!(c.time_source, LapTimeSource::Official);
}

/// The production path (`TimingSample::from_frame`) must keep the grid lap's `before_line` flag even though
/// the normalizer wraps the negative scoring distance into the lap progress (A-LMU-070).
#[test]
fn grid_lap_from_frames_is_before_line() {
    let frame = |et: f64, lap: i32, start: f64, scoring_d: f64, progress: f64| {
        let mut f = TelemetryFrame::new(1, 0, Some(et));
        f.set_valid(ch::LAP_NUMBER, Value::I32(lap)).expect("set");
        f.set_valid(ch::LAP_START_TIME, Value::F64(start))
            .expect("set");
        f.set_valid(ch::LAP_DISTANCE, Value::F64(scoring_d))
            .expect("set");
        #[allow(clippy::cast_possible_truncation)]
        f.set_valid(ch::LAP_PROGRESS, Value::F32(progress as f32))
            .expect("set");
        f.set_valid(ch::TRACK_LENGTH, Value::F64(L)).expect("set");
        f.set_valid(ch::IN_PIT_LANE, Value::Bool(false))
            .expect("set");
        TimingSample::from_frame(&f).expect("sample")
    };
    let mut eng = TimingEngine::new(TimingConfig::default()).expect("engine");
    eng.set_context(key(1));
    let mut completed = Vec::new();
    // Grid: 150 m behind the line. Scoring says -150 m, the progress is wrapped to (L - 150) / L.
    for i in 0..50 {
        let s = frame(50.0 + 0.1 * f64::from(i), 0, 0.0, -150.0, (L - 150.0) / L);
        assert!(s.lap_dist_m.is_some_and(|d| d > 0.0), "progress is wrapped");
        completed.extend(eng.update(&s).completed_lap);
    }
    // Race start: scoring re-bases to L - 150, the car rolls to the line.
    for i in 0..50 {
        let d = L - 150.0 + 3.0 * f64::from(i);
        completed.extend(
            eng.update(&frame(60.0 + 0.1 * f64::from(i), 0, 0.0, d, d / L))
                .completed_lap,
        );
    }
    let line = 65.0;
    let lap_time = 100.0;
    let mut et = line;
    while et < line + lap_time {
        let d = L * (et - line) / lap_time;
        completed.extend(eng.update(&frame(et, 1, line, d, d / L)).completed_lap);
        et += 0.1;
    }
    let line2 = line + lap_time;
    for i in 0..30 {
        let et = line2 + 0.1 * f64::from(i);
        let d = L * (et - line2) / lap_time;
        completed.extend(eng.update(&frame(et, 2, line2, d, d / L)).completed_lap);
    }
    assert_eq!(completed.len(), 2, "{completed:?}");
    assert!(completed[0].flags.before_line);
    assert!(!completed[0].eligible_for_best());
    assert!(!completed[1].flags.before_line);
}

// ---------------------------------------------------------------------------------------------------------
// DEFERRED 19: a lap ended by a garage teleport is never a best or a reference
// ---------------------------------------------------------------------------------------------------------

/// Real Gate 1 pitstop1 timing of the return to the garage (LMU 1.4.2.0, probe
/// session-62bdcac0/normfix + vefix): ET 315.7925 and 315.8025 before the jump, the 19.17 m jump at ET
/// 315.8304 with the lap counter 1 -> 2 on that very sample (lap 1 then measured 105.43 s, so it started
/// at ET 210.4004), the dropped 0.0104 s step-back to 315.8200, then ET 315.8400 and 315.8600.
const PITSTOP1_LAP1_START_ET: f64 = 315.8304 - 105.43;
const PITSTOP1_BEFORE_JUMP_ET: [f64; 2] = [315.7925, 315.8025];
const PITSTOP1_JUMP_ET: f64 = 315.8304;
const PITSTOP1_AFTER_STEP_BACK_ET: [f64; 2] = [315.8400, 315.8600];

/// Drives lap 1 of the pitstop1 pattern **on track** (no pit flag anywhere, the hazard DEFERRED 19
/// describes) and ends it on the teleport sample. `jump` is what `source.discontinuity` says there.
fn pitstop1_lap_ended_by_teleport(jump: Option<CarDiscontinuity>) -> (TimingEngine, LapRecord) {
    let mut eng = TimingEngine::new(TimingConfig::default()).expect("config");
    eng.set_context(key(1));
    let on_track = |et: f64, lap: i32, start: f64, d: f64| TimingSample {
        lap_number: Some(lap),
        lap_start_et_s: Some(start),
        lap_dist_m: Some(d),
        track_length_m: Some(L),
        sector_index: Some(three_sectors(d)),
        lap_invalidated: Some(false),
        in_pit_lane: Some(false),
        discontinuity: Some(CarDiscontinuity::None),
        ..TimingSample::new(et)
    };
    // The end of lap 0 (pitstop1: the pit stop, so an out-lap that is never a best), then lap 1 from the
    // line (counter, lap start and wrap agree: a trusted start).
    eng.update(&TimingSample {
        in_pit_lane: Some(true),
        ..on_track(PITSTOP1_LAP1_START_ET - 0.01, 0, 100.0, L - 0.6)
    });
    let lap_end = PITSTOP1_BEFORE_JUMP_ET[0];
    let n = 2_000;
    for k in 0..n {
        let frac = f64::from(k) / f64::from(n);
        let et = PITSTOP1_LAP1_START_ET + frac * (lap_end - PITSTOP1_LAP1_START_ET);
        eng.update(&on_track(et, 1, PITSTOP1_LAP1_START_ET, frac * (L - 300.0)));
    }
    for et in PITSTOP1_BEFORE_JUMP_ET {
        eng.update(&on_track(et, 1, PITSTOP1_LAP1_START_ET, L - 300.0));
    }
    // The jump into the garage: counter 1 -> 2 and a new LMU lap start on the same sample, pit area.
    let teleport = TimingSample {
        in_pit_lane: Some(true),
        discontinuity: jump,
        ..on_track(PITSTOP1_JUMP_ET, 2, PITSTOP1_JUMP_ET, 120.0)
    };
    let lap = eng
        .update(&teleport)
        .completed_lap
        .expect("lap 1 completes on the jump");
    // The step-back sample was dropped by the normalizer; the next frame reports it.
    let mut after = TimingSample {
        in_pit_lane: Some(true),
        discontinuity: jump.map(|_| CarDiscontinuity::TeleportStepBack),
        ..on_track(PITSTOP1_AFTER_STEP_BACK_ET[0], 2, PITSTOP1_JUMP_ET, 120.0)
    };
    eng.update(&after);
    after.et_s = PITSTOP1_AFTER_STEP_BACK_ET[1];
    after.discontinuity = Some(CarDiscontinuity::None);
    eng.update(&after);
    (eng, lap)
}

#[test]
fn real_pitstop1_teleport_ended_lap_is_never_a_best_or_reference() {
    // Without the signal (before DEFERRED 19): the jump looks like a line crossing and the 105.43 s lap,
    // which never reached the line, becomes the personal best.
    let (blind, lap) = pitstop1_lap_ended_by_teleport(None);
    assert!((lap.time_s.expect("time") - 105.43).abs() < 1e-9);
    assert!(
        lap.eligible_for_best(),
        "the hazard: no pit flag on the lap"
    );
    assert_eq!(blind.snapshot().personal_best.value, lap.time_s);

    // With `source.discontinuity`: the lap is flagged and excluded from bests, best sectors and references.
    let (eng, lap) = pitstop1_lap_ended_by_teleport(Some(CarDiscontinuity::Teleport));
    assert!(lap.flags.discontinuity);
    assert!(
        !lap.flags.pit_in,
        "the boundary sample belongs to the next lap"
    );
    assert!(lap.time_trusted(), "the timing evidence itself is fine");
    assert!(!lap.eligible_for_best());
    let snap = eng.snapshot();
    assert_eq!(snap.personal_best.value, None);
    assert_eq!(snap.personal_best_lap, None);
    assert!(snap.best_sectors.iter().all(|s| s.value.is_none()));
    assert_eq!(snap.reference_lap, None);
    // The lap started by the jump (out of the garage) is flagged too.
    assert!(
        eng.current
            .as_ref()
            .is_some_and(|c| c.discontinuity && c.lap_number == Some(2))
    );
}

#[test]
fn a_discontinuity_inside_a_lap_flags_that_lap() {
    let mut sim = Sim::new(TimingConfig::default());
    sim.lap_plain(&Profile::uniform(100.0));
    sim.cross();
    // A realtime entry in the middle of the next lap (no lap boundary there).
    let mut s = sim.sample(3_000.0, sim.line_et + 50.0, false, false);
    s.discontinuity = Some(CarDiscontinuity::RealtimeEntry);
    sim.eng.update(&s);
    let mut s = sim.sample(3_010.0, sim.line_et + 50.2, false, false);
    s.discontinuity = Some(CarDiscontinuity::None);
    sim.eng.update(&s);
    sim.line_et += 100.0;
    let up = sim.cross();
    let lap = up.completed_lap.expect("lap completes");
    assert!(lap.flags.discontinuity);
    assert!(!lap.eligible_for_best());
    // The clean first lap stays the personal best.
    assert!((sim.eng.snapshot().personal_best.value.expect("pb") - 100.0).abs() < EPS);
}

#[test]
fn discontinuity_is_read_from_the_frame() {
    let mut f = TelemetryFrame::new(1, 0, Some(315.8304));
    f.set_valid(
        ch::SRC_DISCONTINUITY,
        Value::U8(CarDiscontinuity::Teleport.code()),
    )
    .expect("set");
    let s = TimingSample::from_frame(&f).expect("sample");
    assert_eq!(s.discontinuity, Some(CarDiscontinuity::Teleport));
    let none = TimingSample::from_frame(&TelemetryFrame::new(2, 0, Some(1.0))).expect("sample");
    assert_eq!(none.discontinuity, None, "unknown, not 'none'");
}

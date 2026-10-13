//! Behaviour of the synthetic provider: determinism, plausibility (validator-clean frames), lap timing,
//! pit stops, invalid laps, hybrid / non-hybrid, stalls, malformed values, weather and cautions.

use synthetic::{
    MalformedKind, SYNTHETIC_NAME_PREFIX, Scenario, StaleMode, SyntheticEvent,
    SyntheticTelemetryProvider, SyntheticTick, TestMode, encode_snapshot_with_injections,
    is_synthetic_name,
};
use telemetry_model::ch;
use telemetry_model::channel::ChannelId;
use telemetry_model::enums::{GamePhase, PitState, SurfaceType};
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::validate::{FrameRepeat, FrameValidator, Rejection, classify_repeat};
use telemetry_model::value::{Quality, Value};

fn provider(scenario: Scenario) -> SyntheticTelemetryProvider {
    SyntheticTelemetryProvider::new(TestMode::acknowledge(), scenario).expect("valid scenario")
}

fn tick(p: &mut SyntheticTelemetryProvider) -> SyntheticTick {
    p.next_tick().expect("tick")
}

fn frame(t: &SyntheticTick) -> &TelemetryFrame {
    t.frame.as_ref().expect("frame")
}

fn num(f: &TelemetryFrame, id: ChannelId) -> f64 {
    f.f64(id)
        .unwrap_or_else(|| panic!("channel 0x{:04X} not valid: {:?}", id.0, f.get(id)))
}

fn text(f: &TelemetryFrame, id: ChannelId) -> String {
    match &f.get(id).value {
        Value::Str(s) => s.clone(),
        other => panic!("channel 0x{:04X} is not a string: {other:?}", id.0),
    }
}

fn player_laps(t: &SyntheticTick) -> impl Iterator<Item = &synthetic::car::CompletedLap> {
    t.events.iter().filter_map(|e| match e {
        SyntheticEvent::LapCompleted {
            is_player: true,
            lap,
            ..
        } => Some(lap),
        _ => None,
    })
}

/// Runs until the player completed `laps` laps (bounded), validating every frame; returns the laps.
fn run_laps(
    p: &mut SyntheticTelemetryProvider,
    laps: usize,
    mut each: impl FnMut(&SyntheticTick),
) -> Vec<synthetic::car::CompletedLap> {
    let max_ticks =
        ((laps as f64 + 2.0) * 1.3 * p.reference_lap_time_s() * p.scenario().telemetry_rate_hz)
            as usize;
    let mut validator = FrameValidator::new();
    let mut done = Vec::new();
    for _ in 0..max_ticks {
        let mut t = tick(p);
        done.extend(player_laps(&t).copied());
        if let Some(f) = t.frame.as_mut() {
            let report = validator.validate(f);
            assert!(
                report.rejected.is_empty(),
                "et {:.2}: validator rejected {:?}",
                t.session_et_s,
                report.rejected
            );
        }
        each(&t);
        if done.len() >= laps {
            return done;
        }
    }
    panic!("only {} laps completed", done.len());
}

#[test]
fn same_seed_gives_identical_ticks_and_other_seeds_differ() {
    let mut a = provider(Scenario::demo(42));
    let mut b = provider(Scenario::demo(42));
    for _ in 0..3_000 {
        assert_eq!(tick(&mut a), tick(&mut b));
    }
    let mut c = provider(Scenario::demo(43));
    let mut d = provider(Scenario::demo(42));
    let differs = (0..3_000).any(|_| tick(&mut c).frame != tick(&mut d).frame);
    assert!(differs);
}

#[test]
fn every_tick_is_labelled_synthetic() {
    let mut p = provider(Scenario::normal_lap(1));
    assert!(p.origin().is_synthetic());
    let id = p.session_identity();
    for name in [
        &id.vehicle_name,
        &id.vehicle_model,
        &id.class_name,
        &id.track_name,
    ] {
        assert!(is_synthetic_name(name.as_deref().expect("name")));
    }
    for _ in 0..50 {
        let t = tick(&mut p);
        assert!(t.origin.is_synthetic());
        let f = frame(&t);
        for idc in [
            ch::VEHICLE_NAME,
            ch::VEHICLE_MODEL,
            ch::VEHICLE_CLASS_NAME,
            ch::VEHICLE_FILENAME,
            ch::TRACK_NAME,
        ] {
            assert!(text(f, idc).starts_with(SYNTHETIC_NAME_PREFIX), "{idc:?}");
        }
        if let Some(s) = &t.scoring {
            for v in &s.state.vehicles {
                assert!(is_synthetic_name(v.driver.as_deref().expect("driver")));
                assert!(is_synthetic_name(
                    v.vehicle_name.as_deref().expect("vehicle")
                ));
            }
        }
    }
}

#[test]
fn default_rate_frames_are_plausible_and_validator_clean() {
    let mut p = provider(Scenario::normal_lap(7));
    assert_eq!(p.scenario().telemetry_rate_hz, 100.0);
    let v = p.vehicle().clone();
    let mut last_fuel = f64::INFINITY;
    let mut max_speed: f64 = 0.0;
    let mut max_g: f64 = 0.0;
    let mut gears = std::collections::BTreeSet::new();
    let mut tyre_first: Option<(f64, f64)> = None;
    let mut tyre_last = (0.0, 0.0);
    let mut max_brake_temp: f64 = 0.0;
    let laps = run_laps(&mut p, 2, |t| {
        let f = frame(t);
        let tyre = (num(f, ch::TYRE_CARCASS_TEMP_FL), num(f, ch::TYRE_WEAR_FL));
        tyre_first.get_or_insert(tyre);
        tyre_last = tyre;
        max_brake_temp = max_brake_temp.max(num(f, ch::BRAKE_TEMP_FL));
        let speed = num(f, ch::SPEED);
        assert!((0.0..=v.max_speed_mps + 0.5).contains(&speed), "{speed}");
        let rpm = num(f, ch::RPM);
        assert!(rpm >= v.idle_rpm - 1.0 && rpm <= v.max_rpm + 1.0, "{rpm}");
        let gear = num(f, ch::GEAR);
        gears.insert(gear as i64);
        for id in [ch::THROTTLE, ch::BRAKE] {
            assert!((0.0..=1.0).contains(&num(f, id)));
        }
        let fuel = num(f, ch::FUEL);
        assert!(fuel <= last_fuel, "fuel rose without a stop");
        last_fuel = fuel;
        max_speed = max_speed.max(speed);
        max_g = max_g.max(num(f, ch::G_LAT).abs());
        for (def, sample) in f.iter_valid() {
            if let Some(x) = sample.value.as_f64() {
                assert!(x.is_finite(), "{}", def.name);
            }
        }
        // G forces follow speed and curvature (no noise-free zeros while moving).
        assert!(f.get(ch::G_LONG).quality == Quality::Valid);
    });
    assert_eq!(laps.len(), 2);
    assert!(max_speed > 80.0, "{max_speed}");
    assert!(max_g > 2.0 && max_g < 5.0, "{max_g}");
    assert!(gears.len() >= 5, "{gears:?}");
    // Cold tyres warm up and wear; brakes heat under braking.
    let (first, last) = (tyre_first.expect("frames"), tyre_last);
    assert!(last.0 > first.0 + 20.0, "carcass {first:?} -> {last:?}");
    assert!(
        last.1 < first.1 && last.1 > 0.9,
        "wear {first:?} -> {last:?}"
    );
    assert!(max_brake_temp > 400.0, "{max_brake_temp}");
}

#[test]
fn lap_and_sector_timing_is_consistent() {
    let mut p = provider(
        Scenario::builder(3)
            .name("timing")
            .telemetry_rate_hz(20.0)
            .build()
            .expect("scenario"),
    );
    let reference = p.reference_lap_time_s();
    let mut last_frame: Option<TelemetryFrame> = None;
    let mut lap_number_jumps = Vec::new();
    let mut fuel_at_line = Vec::new();
    let fuel_per_lap = p.vehicle().fuel_per_lap_l;
    let laps = run_laps(&mut p, 4, |t| {
        let f = frame(t);
        if let Some(prev) = &last_frame {
            let (a, b) = (num(prev, ch::LAP_NUMBER), num(f, ch::LAP_NUMBER));
            if b != a {
                lap_number_jumps.push((a, b));
                fuel_at_line.push(num(f, ch::FUEL));
            }
        }
        last_frame = Some(f.clone());
    });
    for (k, lap) in laps.iter().enumerate() {
        assert_eq!(lap.lap as usize, k + 1);
        assert!(lap.valid);
        assert!(
            (lap.lap_time_s - reference).abs() < 0.04 * reference,
            "lap {} {} vs {reference}",
            lap.lap,
            lap.lap_time_s
        );
        let sectors = lap.sectors_s.expect("sectors");
        assert!(sectors.iter().all(|s| *s > 5.0));
        let sum: f64 = sectors.iter().sum();
        assert!((sum - lap.lap_time_s).abs() < 1e-9);
    }
    assert_eq!(
        lap_number_jumps,
        vec![(0.0, 1.0), (1.0, 2.0), (2.0, 3.0), (3.0, 4.0), (4.0, 5.0)]
    );
    // Fuel burn per lap is close to nominal and varies from lap to lap.
    let burns: Vec<f64> = fuel_at_line.windows(2).map(|w| w[0] - w[1]).collect();
    assert_eq!(burns.len(), 4);
    for burn in &burns {
        assert!(
            (0.85 * fuel_per_lap..1.15 * fuel_per_lap).contains(burn),
            "{burns:?} vs {fuel_per_lap}"
        );
    }
    let spread = burns.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - burns.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(spread > 1e-3, "{burns:?}");
    // After the next scoring updates the frame carries the scoring times of the completed laps.
    for _ in 0..10 {
        tick(&mut p);
    }
    let t = tick(&mut p);
    let f = frame(&t);
    let best = laps
        .iter()
        .map(|l| l.lap_time_s)
        .fold(f64::INFINITY, f64::min);
    assert!((num(f, ch::BEST_LAP_TIME) - best).abs() < 1e-9);
    assert!((num(f, ch::LAST_LAP_TIME) - laps[3].lap_time_s).abs() < 1e-9);
    assert!((num(f, ch::LAST_SECTOR1) - laps[3].sectors_s.expect("sectors")[0]).abs() < 1e-9);
    // Delta to best is available from the second lap on and small on a consistent lap.
    assert!(num(f, ch::DELTA_BEST).abs() < 3.0);
}

/// Regression (review pass 2, F2/F3): at 2-5 Hz DELTA_BEST was measured against an older, slower lap (or
/// missing), and sector times could be fabricated as lap / 3.
#[test]
fn low_rate_delta_best_refers_to_the_best_lap() {
    const LAPS: usize = 5;
    for rate in [2.0, 5.0] {
        for seed in 1..=4 {
            let mut p = provider(
                Scenario::builder(seed)
                    .name("lowrate")
                    .telemetry_rate_hz(rate)
                    .scoring_rate_hz(rate.min(5.0))
                    .build()
                    .expect("scenario"),
            );
            let max_ticks = ((LAPS as f64 + 2.0) * 1.3 * p.reference_lap_time_s() * rate) as usize;
            let mut prev_delta: Option<f64> = None;
            let mut best: Option<f64> = None;
            let mut laps = 0;
            for _ in 0..max_ticks {
                let t = tick(&mut p);
                for lap in player_laps(&t) {
                    let sectors = lap.sectors_s.expect("sectors measured");
                    assert!(
                        (sectors[0] - lap.lap_time_s / 3.0).abs() > 1e-6,
                        "{rate} Hz seed {seed}: {lap:?}"
                    );
                    if let Some(best) = best {
                        // Just before the line the delta is this lap's time minus the best lap's time.
                        let delta = prev_delta.unwrap_or_else(|| {
                            panic!("{rate} Hz seed {seed} lap {}: DELTA_BEST missing", lap.lap)
                        });
                        let expected = lap.lap_time_s - best;
                        assert!(
                            (delta - expected).abs() < 0.05,
                            "{rate} Hz seed {seed} lap {}: delta {delta:.3} vs {expected:.3}",
                            lap.lap
                        );
                    }
                    if lap.valid {
                        best = Some(best.map_or(lap.lap_time_s, |b: f64| b.min(lap.lap_time_s)));
                    }
                    laps += 1;
                }
                prev_delta = frame(&t).f64(ch::DELTA_BEST);
                if laps >= LAPS {
                    break;
                }
            }
            assert!(laps >= LAPS, "{rate} Hz seed {seed}: {laps} laps");
        }
    }
}

#[test]
fn scoring_arrives_at_the_scoring_rate_with_a_consistent_field() {
    let mut p = provider(Scenario::normal_lap(5));
    let with_scoring: Vec<u64> = (0..201)
        .filter_map(|_| {
            let t = tick(&mut p);
            t.scoring.is_some().then_some(t.tick)
        })
        .collect();
    assert_eq!(with_scoring, (0..=10).map(|k| k * 20).collect::<Vec<_>>());
    let snap = p.latest_scoring();
    let n = p.scenario().competitors + 1;
    assert_eq!(snap.state.vehicles.len(), n);
    let mut places: Vec<u8> = snap
        .state
        .vehicles
        .iter()
        .map(|v| v.place.expect("place"))
        .collect();
    places.sort_unstable();
    assert_eq!(places, (1..=n as u8).collect::<Vec<_>>());
    let player = snap.player().expect("player row");
    assert!(player.is_player);
    assert_eq!(player.place, Some(p.scenario().player_start_place));
    assert!(snap.gaps.place_ahead_s.is_some_and(|g| g > 0.0));
    assert!(snap.gaps.place_behind_s.is_some_and(|g| g > 0.0));
}

#[test]
fn pit_stop_refuels_changes_tyres_and_respects_rate_limits() {
    let mut p = provider(
        Scenario::builder(11)
            .name("pit")
            .telemetry_rate_hz(20.0)
            .start(synthetic::scenario::StartSpec {
                fuel_l: Some(40.0),
                tyre_temp_c: 35.0,
                virtual_energy_frac: 0.5,
                battery_soc_frac: 0.6,
            })
            .pit_stop(synthetic::PitStopSpec {
                lap: 1,
                refuel_l: None,
                change_tyres: true,
                compound: Some(telemetry_model::enums::CompoundType::Soft),
            })
            .build()
            .expect("scenario"),
    );
    let v = p.vehicle().clone();
    let limit = p.track().pit().speed_limit_mps;
    let player_slot = i32::from(p.scenario().player_start_place) - 1;
    let mut states = Vec::new();
    let mut service = None;
    let mut wear_before = None;
    let mut in_lane_frames = 0;
    let mut exited = false;
    let mut fuel_after = 0.0;
    let mut wear_after = 0.0;
    run_laps(&mut p, 3, |t| {
        let f = frame(t);
        let state = PitState::from_raw(num(f, ch::PIT_STATE) as u8);
        if states.last() != Some(&state) {
            states.push(state);
        }
        if num(f, ch::IN_PIT_LANE) == 1.0 {
            in_lane_frames += 1;
            assert!(num(f, ch::SPEED_LIMITER) == 1.0);
            assert!(num(f, ch::SPEED) <= limit + 0.5, "{}", num(f, ch::SPEED));
        } else if service.is_none() {
            // Wear just before the car turns into the pit lane.
            wear_before = Some(num(f, ch::TYRE_WEAR_FL));
        }
        for e in &t.events {
            match e {
                SyntheticEvent::PitBoxStop { slot_id, service_s } if *slot_id == player_slot => {
                    service = Some(*service_s);
                }
                SyntheticEvent::PitLaneExited { slot_id } if *slot_id == player_slot => {
                    exited = true;
                    fuel_after = num(f, ch::FUEL);
                    wear_after = num(f, ch::TYRE_WEAR_FL);
                    assert_eq!(text(f, ch::TYRE_COMPOUND_FRONT), "Soft");
                }
                _ => {}
            }
        }
    });
    assert!(exited, "player left the pits");
    assert!(service.is_some_and(|s| s > 12.0), "{service:?}");
    assert!(in_lane_frames > 0);
    assert!(fuel_after > v.fuel_capacity_l - 1.0, "{fuel_after}");
    assert!(wear_after > 0.999 && wear_before.is_some_and(|w| w < 1.0));
    assert!(states.contains(&PitState::Requested));
    assert!(states.contains(&PitState::Entering));
    assert!(states.contains(&PitState::Stopped));
    assert!(states.contains(&PitState::Exiting));
}

#[test]
fn invalid_lap_is_flagged_and_excluded_from_best() {
    let mut p = provider(
        Scenario::builder(9)
            .name("invalid")
            .telemetry_rate_hz(20.0)
            .invalid_lap(2)
            .build()
            .expect("scenario"),
    );
    let mut saw_flag = false;
    let mut saw_grass = false;
    let mut steps = 0.0;
    let laps = run_laps(&mut p, 3, |t| {
        let f = frame(t);
        if num(f, ch::LAP_INVALID) == 1.0 {
            saw_flag = true;
            assert_eq!(num(f, ch::LAP_NUMBER), 2.0);
        }
        saw_grass |= num(f, ch::SURFACE_TYPE_FL) == f64::from(SurfaceType::Grass.raw());
        steps = num(f, ch::TRACK_LIMITS_STEPS);
    });
    assert!(saw_flag && saw_grass);
    assert_eq!(steps, 1.0);
    assert!(laps[0].valid && !laps[1].valid && laps[2].valid);
    let t = (0..12).map(|_| tick(&mut p)).last().expect("tick");
    let best = num(frame(&t), ch::BEST_LAP_TIME);
    assert!((best - laps[0].lap_time_s.min(laps[2].lap_time_s)).abs() < 1e-9);
}

#[test]
fn hybrid_and_non_hybrid_profiles() {
    let mut hy = provider(Scenario::hybrid(1));
    let mut gt = provider(Scenario::non_hybrid(1));
    let (th, tg) = (tick(&mut hy), tick(&mut gt));
    let (fh, fg) = (frame(&th), frame(&tg));
    for id in [
        ch::BATTERY_SOC,
        ch::HYBRID_MOTOR_RPM,
        ch::REGEN_POWER,
        ch::TURBO_BOOST,
    ] {
        assert_eq!(fh.get(id).quality, Quality::Valid, "{id:?}");
        assert_eq!(fg.get(id).quality, Quality::NotSupported, "{id:?}");
    }
    assert_eq!(fg.get(ch::HYBRID_MOTOR_STATE).quality, Quality::Valid);
    assert_eq!(fh.get(ch::ABS_ACTIVE).quality, Quality::NotSupported);
    assert_eq!(fg.get(ch::ABS_ACTIVE).quality, Quality::Valid);
    // The hybrid deploys and recovers energy over a lap.
    let mut states = std::collections::BTreeSet::new();
    for _ in 0..9_000 {
        let t = tick(&mut hy);
        states.insert(num(frame(&t), ch::HYBRID_MOTOR_STATE) as u8);
    }
    assert!(states.len() >= 3, "{states:?}");
}

#[test]
fn stalls_freeze_session_time_and_repeat_or_drop_frames() {
    let mut p = provider(Scenario::stale(2));
    let rate = p.scenario().telemetry_rate_hz;
    let mut prev: Option<SyntheticTick> = None;
    let (mut frozen, mut missing) = (0, 0);
    let mut resumed_after_freeze = false;
    for _ in 0..(50.0 * rate) as usize {
        let t = tick(&mut p);
        if let Some(prev) = &prev {
            assert!(t.capture_mono_ns > prev.capture_mono_ns);
            match t.stale {
                Some(StaleMode::FrozenFrames) => {
                    frozen += 1;
                    let (a, b) = (prev.frame.as_ref().expect("prev"), frame(&t));
                    assert_eq!(classify_repeat(a, b), FrameRepeat::Duplicate);
                    assert!(b.sequence > a.sequence);
                    assert_eq!(t.session_et_s, prev.session_et_s);
                    assert!(t.scoring.is_none());
                }
                Some(StaleMode::NoFrames) => {
                    missing += 1;
                    assert!(t.frame.is_none());
                    assert_eq!(t.session_et_s, prev.session_et_s);
                }
                None => {
                    if prev.stale.is_some() {
                        // Session time continues where it froze: one step, no jump.
                        assert!((t.session_et_s - prev.session_et_s - 1.0 / rate).abs() < 1e-9);
                        resumed_after_freeze = true;
                    }
                }
            }
        }
        prev = Some(t);
    }
    assert_eq!(frozen, (3.0 * rate) as usize);
    assert_eq!(missing, (2.0 * rate) as usize);
    assert!(resumed_after_freeze);
}

#[test]
fn malformed_values_only_touch_the_injected_channel() {
    let mut bad = provider(Scenario::malformed(21));
    let mut clean = provider(Scenario::normal_lap(21));
    let mut validator = FrameValidator::new();
    let mut kinds = std::collections::BTreeSet::new();
    let mut nan_wire_checked = false;
    for _ in 0..6_000 {
        let (tb, tc) = (tick(&mut bad), tick(&mut clean));
        let (mut fb, fc) = (frame(&tb).clone(), frame(&tc));
        match tb.injection {
            None => assert_eq!(&fb, fc),
            Some(inj) => {
                kinds.insert(format!("{:?}", inj.kind));
                for ((def, a), (_, b)) in fb.iter().zip(fc.iter()) {
                    if def.id != inj.channel {
                        assert_eq!(a, b, "{}", def.name);
                    }
                }
                match inj.kind {
                    MalformedKind::NaN | MalformedKind::Infinity => {
                        assert!(!inj.raw_value.is_finite());
                        assert_eq!(fb.get(inj.channel).quality, Quality::Invalid);
                        if !nan_wire_checked {
                            let mut buf = Vec::new();
                            encode_snapshot_with_injections(&fb, &[inj], &mut buf, 0)
                                .expect("encode");
                            let (decoded, report) =
                                TelemetryFrame::from_snapshot(fb.sequence, &buf).expect("decode");
                            assert!(report.invalidated >= 1);
                            assert_ne!(decoded.get(inj.channel).quality, Quality::Valid);
                            nan_wire_checked = true;
                        }
                    }
                    MalformedKind::OutOfRange => {
                        let report = validator.validate(&mut fb);
                        assert!(report.rejected.iter().any(|(id, r)| *id == inj.channel
                            && matches!(r, Rejection::OutOfRange { .. })));
                        continue;
                    }
                    MalformedKind::Spike => {}
                }
            }
        }
        validator.validate(&mut fb);
    }
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    assert!(nan_wire_checked);
}

#[test]
fn rain_wets_the_track_and_slows_the_laps() {
    let mut p = provider(
        Scenario::builder(4)
            .name("rain")
            .telemetry_rate_hz(10.0)
            .rain(synthetic::RainSpec {
                start_et_s: 0.0,
                ramp_s: 30.0,
                duration_s: 3_000.0,
                intensity: 0.9,
            })
            .build()
            .expect("scenario"),
    );
    let mut wiper = 0.0_f64;
    let mut wet = 0.0_f64;
    let laps = run_laps(&mut p, 3, |t| {
        let f = frame(t);
        wiper = wiper.max(num(f, ch::WIPER_STATE));
        wet = wet.max(num(f, ch::TRACK_WETNESS));
    });
    assert!(wet > 0.6, "{wet}");
    assert_eq!(wiper, 2.0);
    let dry = p.reference_lap_time_s();
    assert!(
        laps[2].lap_time_s > dry + 5.0,
        "{} vs {dry}",
        laps[2].lap_time_s
    );
}

#[test]
fn safety_car_caps_speed_and_local_yellow_marks_a_sector() {
    let mut p = provider(
        Scenario::builder(6)
            .name("sc")
            .telemetry_rate_hz(10.0)
            .caution(synthetic::CautionSpec {
                kind: synthetic::CautionKind::SafetyCar,
                start_et_s: 30.0,
                duration_s: 120.0,
            })
            .caution(synthetic::CautionSpec {
                kind: synthetic::CautionKind::LocalYellow { sector: 1 },
                start_et_s: 200.0,
                duration_s: 30.0,
            })
            .build()
            .expect("scenario"),
    );
    let cap = synthetic::session::SAFETY_CAR_SPEED_MPS;
    let mut saw_fcy = false;
    let mut saw_local = false;
    let mut validator = FrameValidator::new();
    for _ in 0..2_150 {
        let mut t = tick(&mut p);
        let et = t.session_et_s;
        let f = t.frame.as_mut().expect("frame");
        assert!(validator.validate(f).rejected.is_empty());
        if (60.0..145.0).contains(&et) {
            saw_fcy = true;
            assert_eq!(
                num(f, ch::GAME_PHASE),
                f64::from(GamePhase::FullCourseYellow.raw())
            );
            assert_eq!(num(f, ch::FULL_COURSE_YELLOW), 1.0);
            assert!(
                num(f, ch::SPEED) <= cap + 0.5,
                "et {et}: {}",
                num(f, ch::SPEED)
            );
        }
        if (201.0..229.0).contains(&et) {
            saw_local = true;
            assert_eq!(num(f, ch::SECTOR_FLAG_1), 1.0);
            assert_eq!(num(f, ch::SECTOR_FLAG_0), 11.0);
        }
    }
    assert!(saw_fcy && saw_local);
}

#[tokio::test]
async fn realtime_stream_ticks_into_a_bounded_queue() {
    let p = provider(
        Scenario::builder(1)
            .name("rt")
            .telemetry_rate_hz(200.0)
            .scoring_rate_hz(5.0)
            .build()
            .expect("scenario"),
    );
    assert!(p.clone().into_realtime(0).is_err());
    let mut stream = p.into_realtime(8).expect("stream");
    for expected in 0..10 {
        let t = stream.recv().await.expect("tick");
        assert_eq!(t.tick, expected);
        assert!(t.origin.is_synthetic());
    }
    assert_eq!(stream.dropped(), 0);
    stream.finish().await.expect("a normal stop is not an error");
}

fn realtime_provider(rate_hz: f64) -> SyntheticTelemetryProvider {
    provider(
        Scenario::builder(1)
            .name("rt")
            .telemetry_rate_hz(rate_hz)
            .build()
            .expect("scenario"),
    )
}

fn ns_since(epoch: std::time::Instant) -> u64 {
    u64::try_from(epoch.elapsed().as_nanos()).expect("ns")
}

/// Regression (review pass 2, F1/F11): the stream used to fall ~35 % behind its rate on Windows timers and
/// stamped ticks with a simulated clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_stream_keeps_pace_with_wall_time_and_stamps_real_time() {
    const RATE_HZ: f64 = 100.0;
    const RUN: std::time::Duration = std::time::Duration::from_secs(2);
    let epoch = std::time::Instant::now();
    let mut stream = realtime_provider(RATE_HZ)
        .into_realtime_with_epoch(1_024, epoch)
        .expect("stream");
    let start = std::time::Instant::now();
    let mut received = 0_u64;
    let mut last: Option<SyntheticTick> = None;
    while start.elapsed() < RUN {
        let t = tokio::time::timeout(std::time::Duration::from_millis(500), stream.recv())
            .await
            .expect("a tick within 0.5 s")
            .expect("stream open");
        if let Some(prev) = &last {
            assert_eq!(t.tick, prev.tick + 1);
            assert!(t.capture_mono_ns > prev.capture_mono_ns);
        }
        assert_eq!(t.capture_mono_ns, frame(&t).capture_mono_ns);
        received += 1;
        last = Some(t);
    }
    let elapsed_s = start.elapsed().as_secs_f64();
    let expected = elapsed_s * RATE_HZ;
    let produced = received + stream.dropped();
    assert!(
        produced as f64 >= 0.9 * expected && produced as f64 <= 1.1 * expected + 2.0,
        "{produced} ticks in {elapsed_s:.3} s at {RATE_HZ} Hz"
    );
    let last = last.expect("ticks");
    // Real-clock stamps: the last tick is fresh in the caller's time base.
    let age_ns = ns_since(epoch).saturating_sub(last.capture_mono_ns);
    assert!(age_ns < 100_000_000, "last tick is {age_ns} ns old");
    assert!(last.session_et_s >= 0.9 * elapsed_s, "{}", last.session_et_s);
    stream.stop();
}

/// Regression (review pass 2, F10): a full queue used to discard the newest tick and deliver stale ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn realtime_full_queue_discards_the_oldest_ticks() {
    let epoch = std::time::Instant::now();
    let mut stream = realtime_provider(100.0)
        .into_realtime_with_epoch(1, epoch)
        .expect("stream");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let t = stream.recv().await.expect("tick");
    assert!(stream.dropped() >= 15, "dropped {}", stream.dropped());
    assert!(t.tick >= 15, "delivered tick {}", t.tick);
    let age_ns = ns_since(epoch).saturating_sub(t.capture_mono_ns);
    assert!(
        age_ns < 100_000_000,
        "delivered tick is {age_ns} ns old: not the newest"
    );
    stream.finish().await.expect("normal stop");
}

#[test]
fn realtime_needs_a_runtime() {
    let p = provider(Scenario::normal_lap(1));
    assert!(matches!(
        p.into_realtime(4),
        Err(synthetic::SyntheticError::NoRuntime(_))
    ));
}

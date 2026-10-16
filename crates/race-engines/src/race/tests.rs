//! Race engine tests.

use telemetry_model::enums::{CarFlag, GamePhase, PitState, YellowFlagState};
use telemetry_model::events::{RaceFlag, SessionEvent};
use telemetry_model::scoring::{CompetitorEntry, ScoringState};

use super::*;

fn pit_in(et: f64, ps: Option<PitState>, lane: Option<bool>, speed: f64) -> PitInputs {
    PitInputs {
        et_s: et,
        pit_state: ps,
        in_pits: None,
        in_pit_lane: lane,
        in_garage: Some(false),
        speed_mps: Some(speed),
    }
}

fn events(m: &mut PitStateMachine, steps: &[PitInputs]) -> Vec<PitEventRecord> {
    steps.iter().flat_map(|s| m.update(s)).collect()
}

#[test]
fn pit_cycle_from_lmu_state_with_lane_bit_timing() {
    let mut m = PitStateMachine::default();
    let ev = events(
        &mut m,
        &[
            pit_in(10.0, Some(PitState::Idle), Some(false), 60.0),
            // Telemetry lane bit first (scoring lags): entry at 11.0.
            pit_in(11.0, Some(PitState::Idle), Some(true), 22.0),
            pit_in(11.2, Some(PitState::Entering), Some(true), 22.0),
            pit_in(30.0, Some(PitState::Stopped), Some(true), 0.0),
            pit_in(55.0, Some(PitState::Exiting), Some(true), 5.0),
            // Lane bit leaves before the scoring state: exit at 70.0.
            pit_in(70.0, Some(PitState::Exiting), Some(false), 25.0),
            pit_in(70.2, Some(PitState::Idle), Some(false), 30.0),
        ],
    );
    let kinds: Vec<_> = ev.iter().map(|e| e.event).collect();
    assert_eq!(
        kinds,
        vec![
            PitEvent::Entry { et_s: 11.0 },
            PitEvent::StopStart { et_s: 30.0 },
            PitEvent::StopEnd {
                et_s: 55.0,
                stationary_s: Some(25.0)
            },
            PitEvent::Exit {
                et_s: 70.0,
                lane_time_s: Some(59.0),
                stopped: true
            },
        ]
    );
    assert_eq!(ev[0].source, PitSignalSource::LmuPitLaneFlag);
    assert_eq!(ev[1].source, PitSignalSource::LmuPitState);
    assert!(ev.iter().all(|e| !e.source.is_calculated()));
    assert_eq!(m.phase(), PitPhase::OnTrack);
    assert_eq!(m.stops_observed(), 1);
    let visit = m.last_visit().expect("visit");
    assert_eq!(visit.stationary_s, Some(25.0));
    assert!(!visit.stop_inferred);
    assert_eq!(
        ev[2].to_session_event(),
        Some(SessionEvent::PitStop {
            stationary_s: Some(25.0)
        })
    );
    assert_eq!(ev[0].to_session_event(), Some(SessionEvent::PitEntry));
    assert_eq!(ev[1].to_session_event(), None);
}

#[test]
fn leaving_the_garage_with_lmu_state_5_is_not_a_pit_entry() {
    let mut m = PitStateMachine::default();
    let garage = PitState::from_raw(5);
    let ev = events(
        &mut m,
        &[
            pit_in(0.0, Some(garage), Some(true), 0.0),
            pit_in(1.0, Some(garage), Some(true), 8.0),
            pit_in(30.0, Some(garage), Some(false), 30.0),
        ],
    );
    assert_eq!(ev.len(), 1);
    assert!(matches!(
        ev[0].event,
        PitEvent::Exit {
            stopped: false,
            lane_time_s: None,
            ..
        }
    ));
    assert_eq!(m.phase(), PitPhase::OnTrack);
}

#[test]
fn drive_through_has_no_stop() {
    let mut m = PitStateMachine::default();
    let ev = events(
        &mut m,
        &[
            pit_in(0.0, Some(PitState::Idle), Some(false), 60.0),
            pit_in(1.0, Some(PitState::Entering), Some(true), 22.0),
            pit_in(20.0, Some(PitState::Idle), Some(false), 50.0),
        ],
    );
    assert_eq!(ev.len(), 2);
    assert_eq!(
        ev[1].event,
        PitEvent::Exit {
            et_s: 20.0,
            lane_time_s: Some(19.0),
            stopped: false
        }
    );
    assert_eq!(m.stops_observed(), 0);
}

#[test]
fn lmu_state_wins_over_speed_inference() {
    let mut m = PitStateMachine::default();
    // Stationary in the lane for 10 s while LMU still says Entering (queueing): no inferred stop.
    let steps: Vec<_> = (0..=10)
        .map(|i| pit_in(f64::from(i), Some(PitState::Entering), Some(true), 0.0))
        .collect();
    let ev = events(&mut m, &steps);
    assert_eq!(ev.len(), 1);
    assert!(matches!(ev[0].event, PitEvent::Entry { .. }));
    assert_eq!(m.phase(), PitPhase::Entering);
}

#[test]
fn stop_is_inferred_from_speed_only_without_lmu_pit_state() {
    let mut m = PitStateMachine::default();
    let mut steps = vec![
        pit_in(0.0, None, Some(false), 60.0),
        pit_in(1.0, None, Some(true), 20.0),
    ];
    // Stationary from t = 2: the stop is confirmed at t = 5 but starts at t = 2.
    steps.extend((2..=8).map(|i| pit_in(f64::from(i), None, Some(true), 0.1)));
    steps.push(pit_in(9.0, None, Some(true), 6.0));
    steps.push(pit_in(20.0, None, Some(false), 40.0));
    let ev = events(&mut m, &steps);
    let kinds: Vec<_> = ev.iter().map(|e| (e.event, e.source)).collect();
    assert_eq!(
        kinds,
        vec![
            (
                PitEvent::Entry { et_s: 1.0 },
                PitSignalSource::LmuPitLaneFlag
            ),
            (
                PitEvent::StopStart { et_s: 2.0 },
                PitSignalSource::SpeedInference
            ),
            (
                PitEvent::StopEnd {
                    et_s: 9.0,
                    stationary_s: Some(7.0)
                },
                PitSignalSource::SpeedInference
            ),
            (
                PitEvent::Exit {
                    et_s: 20.0,
                    lane_time_s: Some(19.0),
                    stopped: true
                },
                PitSignalSource::LmuPitLaneFlag
            ),
        ]
    );
    assert!(ev[1].source.is_calculated());
    assert_eq!(ev[1].provenance(), telemetry_model::Provenance::Estimated);
    assert!(m.last_visit().expect("visit").stop_inferred);
}

#[test]
fn garage_exit_starts_a_visit_without_entry() {
    let mut m = PitStateMachine::default();
    let mut garage = pit_in(0.0, Some(PitState::Idle), Some(true), 0.0);
    garage.in_garage = Some(true);
    let ev = events(
        &mut m,
        &[
            garage,
            pit_in(5.0, Some(PitState::Exiting), Some(true), 10.0),
            pit_in(25.0, Some(PitState::Idle), Some(false), 40.0),
        ],
    );
    assert_eq!(ev.len(), 1);
    assert_eq!(
        ev[0].event,
        PitEvent::Exit {
            et_s: 25.0,
            lane_time_s: None,
            stopped: false
        }
    );
}

fn car(slot: i32, place: u8, class: &str) -> CompetitorEntry {
    CompetitorEntry {
        place: Some(place),
        class_name: Some(class.into()),
        laps: Some(10),
        laps_behind_leader: Some(0),
        laps_behind_next: Some(0),
        ..CompetitorEntry::new(slot)
    }
}

fn field() -> ScoringState {
    let mut leader = car(1, 1, "Hypercar");
    leader.time_behind_leader_s = Some(0.0);
    let mut gt = car(2, 2, "LMGT3");
    gt.time_behind_leader_s = Some(3.0);
    gt.time_behind_next_s = Some(3.0);
    let mut player = car(3, 3, "Hypercar");
    player.is_player = true;
    player.time_behind_leader_s = Some(5.5);
    player.time_behind_next_s = Some(2.5);
    player.driver = Some("Driver".into());
    player.pit_group = Some("Team".into());
    player.estimated_lap_s = Some(100.0);
    let mut behind = car(4, 4, "Hypercar");
    behind.time_behind_leader_s = Some(80.0);
    behind.time_behind_next_s = Some(74.5);
    behind.laps_behind_leader = Some(1);
    behind.laps_behind_next = Some(1);
    ScoringState {
        session: telemetry_model::scoring::SessionScoring {
            game_phase: Some(GamePhase::GreenFlag),
            yellow_flag_state: Some(YellowFlagState::NoFlag),
            current_et_s: Some(1_200.0),
            time_remaining_s: Some(250.0),
            ..Default::default()
        },
        vehicles: vec![leader, gt, player, behind],
    }
}

fn sample(et: f64, progress: f64) -> RaceSample {
    RaceSample {
        et_s: et,
        speed_mps: Some(60.0),
        in_pit_lane: Some(false),
        lap_progress: Some(progress),
    }
}

#[test]
fn positions_and_gaps() {
    let mut eng = RaceEngine::new();
    eng.update(&field(), &sample(1_200.0, 0.5));
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(s.position.value, Some(3));
    assert_eq!(s.position.provenance, telemetry_model::Provenance::Measured);
    assert_eq!(s.class_position.value, Some(2));
    assert_eq!(
        s.class_position.provenance,
        telemetry_model::Provenance::Derived
    );
    assert_eq!(s.driver.as_deref(), Some("Driver"));
    assert_eq!(s.team.as_deref(), Some("Team"));
    // Overall: LMU values.
    assert_eq!(s.gap_ahead.seconds, Some(2.5));
    assert_eq!(s.gap_behind.seconds, Some(74.5));
    assert_eq!(s.gap_behind.laps, Some(1));
    assert_eq!(s.gap_behind.other_slot_id, Some(4));
    // Class ahead: the leader, same lap: 5.5 s.
    assert_eq!(s.class_gap_ahead.seconds, Some(5.5));
    assert_eq!(s.class_gap_ahead.laps, Some(0));
    assert_eq!(s.class_gap_ahead.other_slot_id, Some(1));
    assert_eq!(
        s.class_gap_ahead.provenance,
        telemetry_model::Provenance::Estimated
    );
    // Class behind: a lap down, shown in laps only.
    assert_eq!(s.class_gap_behind.laps, Some(1));
    assert_eq!(s.class_gap_behind.seconds, None);
    assert!(s.standings_consistent);
}

#[test]
fn leader_has_no_gap_ahead() {
    let mut f = field();
    for v in &mut f.vehicles {
        v.is_player = v.slot_id == 1;
    }
    let mut eng = RaceEngine::new();
    eng.update(&f, &sample(1_200.0, 0.5));
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(s.gap_ahead.seconds, None);
    assert_eq!(s.class_gap_ahead.seconds, None);
    assert_eq!(s.class_position.value, Some(1));
}

#[test]
fn race_clock_and_laps_remaining() {
    // Timed race: 250 s left at half a lap with a 100 s pace: floor(0.5 + 2.5) + 1 - 0.5 = 3.5 laps.
    let mut eng = RaceEngine::new();
    eng.update(&field(), &sample(1_200.0, 0.5));
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(s.total_laps, None);
    assert_eq!(
        s.race_finish,
        RaceFinish::Time {
            time_remaining_s: 250.0
        }
    );
    assert_eq!(s.race_laps_remaining.value, Some(3.5));
    assert_eq!(s.session_remaining_s.value, Some(250.0));
    assert_eq!(s.session_elapsed_s.value, Some(1_200.0));

    // Lap race: 20 laps, 10 done, a quarter into the 11th.
    let mut f = field();
    f.session.max_laps = Some(20);
    eng.update(&f, &sample(1_201.0, 0.25));
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(s.total_laps, Some(20));
    assert_eq!(s.race_laps_remaining.value, Some(9.75));

    // A configured pace replaces the LMU estimate for timed races.
    eng.set_reference_pace(Some(125.0));
    eng.update(&field(), &sample(1_202.0, 0.0));
    assert_eq!(
        eng.snapshot().expect("snapshot").race_laps_remaining.value,
        Some(3.0)
    );
}

#[test]
fn flags_and_lap_conditions() {
    let mut eng = RaceEngine::new();
    let up = eng.update(&field(), &sample(1.0, 0.1));
    assert_eq!(up.flag_change, None);
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(s.flags.summary(), RaceFlag::Green);
    assert_eq!(s.flags.full_course_yellow, Some(false));

    let mut f = field();
    f.session.game_phase = Some(GamePhase::FullCourseYellow);
    f.session.yellow_flag_state = Some(YellowFlagState::PitsClosed);
    let up = eng.update(&f, &sample(2.0, 0.2));
    assert_eq!(
        up.flag_change,
        Some((RaceFlag::Green, RaceFlag::FullCourseYellow))
    );
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(s.flags.pits_closed, Some(true));
    assert_eq!(s.flags.green, Some(false));
    assert!(eng.lap_conditions().caution);

    // Lap ends under caution: the next lap starts as a caution lap too.
    let done = eng.take_lap_conditions();
    assert!(done.caution);
    assert!(eng.lap_conditions().caution);
    let up = eng.update(&field(), &sample(3.0, 0.3));
    assert_eq!(
        up.flag_change,
        Some((RaceFlag::FullCourseYellow, RaceFlag::Green))
    );
    eng.take_lap_conditions();
    assert!(!eng.lap_conditions().caution);

    let mut f = field();
    if let Some(p) = f.vehicles.iter_mut().find(|v| v.is_player) {
        p.flag = Some(CarFlag::Blue);
    }
    eng.update(&f, &sample(4.0, 0.4));
    assert_eq!(
        eng.snapshot().expect("snapshot").flags.summary(),
        RaceFlag::Blue
    );
    f.session.game_phase = Some(GamePhase::SessionStopped);
    eng.update(&f, &sample(5.0, 0.5));
    assert_eq!(
        eng.snapshot().expect("snapshot").flags.summary(),
        RaceFlag::Red
    );
}

#[test]
fn unknown_scoring_gives_no_values() {
    let mut eng = RaceEngine::new();
    eng.update(&ScoringState::default(), &RaceSample::default());
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(s.position.value, None);
    assert_eq!(s.class_position.value, None);
    assert_eq!(s.laps_completed.value, None);
    assert_eq!(s.race_laps_remaining.value, None);
    assert_eq!(s.race_finish, RaceFinish::Unknown);
    assert_eq!(s.flags.full_course_yellow, None);
    assert_eq!(s.flags.summary(), RaceFlag::None);
    assert_eq!(s.gap_ahead.seconds, None);
}

#[test]
fn best_lap_gaps_outside_races() {
    // Qualifying: LMU classification gaps are withheld by the normalizer; gaps are best-lap differences.
    let mut f = field();
    f.session.session_type = Some(SessionType::Qualifying(1));
    let bests = [
        (1, Some(100.0)),
        (2, Some(110.0)),
        (3, Some(101.25)),
        (4, None),
    ];
    for v in &mut f.vehicles {
        v.time_behind_next_s = None;
        v.laps_behind_next = None;
        v.time_behind_leader_s = None;
        v.laps_behind_leader = None;
        v.best_lap_s = bests.iter().find(|b| b.0 == v.slot_id).and_then(|b| b.1);
    }
    let mut eng = RaceEngine::new();
    eng.update(&f, &sample(600.0, 0.5));
    let s = eng.snapshot().expect("snapshot");
    // Overall ahead is the GT car (place 2) with a slower best: classification lags, no gap.
    assert_eq!(s.gap_ahead.other_slot_id, Some(2));
    assert_eq!(s.gap_ahead.seconds, None);
    assert_eq!(s.gap_ahead.formula, formula::GAP_BEST_LAP);
    // Behind has no best lap: no gap, never 0.
    assert_eq!(s.gap_behind.other_slot_id, Some(4));
    assert_eq!(s.gap_behind.seconds, None);
    // Class ahead: the Hypercar leader, 1.25 s faster.
    assert_eq!(s.class_gap_ahead.other_slot_id, Some(1));
    assert_eq!(s.class_gap_ahead.seconds, Some(1.25));
    assert_eq!(s.class_gap_ahead.laps, None);
    assert_eq!(
        s.class_gap_ahead.provenance,
        telemetry_model::Provenance::Derived
    );

    // With a consistent classification the overall gap is the best-lap difference too.
    for v in &mut f.vehicles {
        if v.slot_id == 2 {
            v.best_lap_s = Some(100.5);
        }
    }
    eng.update(&f, &sample(601.0, 0.5));
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(s.gap_ahead.seconds, Some(0.75));
}

#[test]
fn race_session_uses_lmu_classification_gaps() {
    let mut f = field();
    f.session.session_type = Some(SessionType::Race(1));
    let mut eng = RaceEngine::new();
    eng.update(&f, &sample(1_200.0, 0.5));
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(s.gap_ahead.seconds, Some(2.5));
    assert_eq!(
        s.gap_ahead.provenance,
        telemetry_model::Provenance::Measured
    );
    assert_eq!(s.class_gap_ahead.seconds, Some(5.5));
}

#[test]
fn session_end_rewind_gives_no_negative_stop() {
    // A stationary car without LMU pit state, then LMU rewinds ET after the session end.
    let mut m = PitStateMachine::default();
    let none = |et: f64, speed: f64| PitInputs {
        et_s: et,
        pit_state: None,
        in_pits: None,
        in_pit_lane: Some(true),
        in_garage: Some(false),
        speed_mps: Some(speed),
    };
    let mut ev = events(
        &mut m,
        &[none(3_600.0, 20.0), none(3_601.0, 0.0), none(3_602.0, 0.0)],
    );
    // Rewind to 3 590 s: the stationary timer restarts, no stop is inferred from the old time base.
    ev.extend(events(&mut m, &[none(3_590.0, 0.0), none(3_591.0, 0.0)]));
    assert!(
        !ev.iter()
            .any(|e| matches!(e.event, PitEvent::StopStart { .. })),
        "{ev:?}"
    );
    ev.extend(events(&mut m, &[none(3_594.0, 0.0), none(3_600.0, 5.0)]));
    for e in &ev {
        match e.event {
            PitEvent::StopEnd { stationary_s, .. } => {
                assert!(stationary_s.is_none_or(|s| s >= 0.0), "{e:?}");
            }
            PitEvent::Exit { lane_time_s, .. } => {
                assert!(lane_time_s.is_none_or(|s| s >= 0.0), "{e:?}");
            }
            _ => {}
        }
    }
    let start = ev.iter().find_map(|e| match e.event {
        PitEvent::StopStart { et_s } => Some(et_s),
        _ => None,
    });
    assert_eq!(start, Some(3_590.0));
}

#[test]
fn rewound_session_clock_is_never_negative() {
    let mut f = field();
    f.session.time_remaining_s = Some(-3.0);
    let mut eng = RaceEngine::new();
    eng.update(&f, &sample(1_200.0, 0.5));
    let s = eng.snapshot().expect("snapshot");
    assert_eq!(
        s.race_finish,
        RaceFinish::Time {
            time_remaining_s: 0.0
        }
    );
}

/// LMU's class place can disagree with the local ranking (scoring lag, a row without a place). The class
/// neighbours must be found from the player's rank in the same local ranking.
#[test]
fn class_neighbours_use_the_local_rank_when_lmu_class_place_disagrees() {
    let mut f = field();
    for v in &mut f.vehicles {
        if v.is_player {
            v.class_place = Some(3); // locally 2nd of the Hypercars
        }
    }
    let mut eng = RaceEngine::new();
    eng.update(&f, &sample(1_200.0, 0.5));
    let s = eng.snapshot().expect("snapshot");
    // The displayed class position stays LMU's value.
    assert_eq!(s.class_position.value, Some(3));
    assert_eq!(s.class_gap_ahead.other_slot_id, Some(1));
    assert_eq!(s.class_gap_ahead.seconds, Some(5.5));
    assert_eq!(s.class_gap_behind.other_slot_id, Some(4));
    assert_eq!(s.class_gap_behind.laps, Some(1));
}

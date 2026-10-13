//! Race engine (spec §20–21, §35, §66, §145): position, class position, gaps, race and session clock, lap
//! count, pit state machine and flags, from LMU scoring plus a few telemetry-rate inputs.
//!
//! # Pit detection (spec §66)
//! LMU signals are used whenever they exist, in this order: the garage flag, the scoring pit state
//! (`Entering`, `Stopped`, `Exiting`), the pit-lane bit of the telemetry sector (higher rate, used for
//! precise entry and exit times) and `mInPits`. Only when no LMU signal says whether the car is stopped is
//! a stop inferred from speed (stationary below [`PIT_STOP_MAX_SPEED_MPS`] for at least
//! [`PIT_STOP_MIN_DURATION_S`]); such events carry [`PitSignalSource::SpeedInference`], are labelled
//! `CALCULATED` and have `Estimated` provenance.
//!
//! # Positions and gaps
//! Overall position and the gaps to the cars directly ahead / behind are LMU scoring values (`Measured`).
//! The class position is ranked by overall place within the class (`Derived`); class gaps are differences
//! of the time behind the leader when both cars are on the same lap (`Estimated`), otherwise a lap count.
//! Race position is never used as a physical track position (spec §22).

use serde::{Deserialize, Serialize};
use telemetry_model::enums::{
    CarFlag, FinishStatus, GamePhase, IndividualPhase, PitState, SessionType, YellowFlagState,
};
use telemetry_model::events::{RaceFlag, SessionEvent};
use telemetry_model::scoring::{CompetitorEntry, ScoringState, lmu_session_time_remaining};
use telemetry_model::{Provenance, TelemetryFrame, ch};
use tracing::debug;

use crate::common::{Metric, finite, formula, frame_bool, frame_f64};
use crate::fuel::RaceFinish;

/// Tracing component name.
const COMPONENT: &str = "race_engines::race";

/// Speed (m/s) below which the car counts as stationary for stop inference.
pub const PIT_STOP_MAX_SPEED_MPS: f64 = 0.5;
/// Minimum stationary time (s) for an inferred stop.
pub const PIT_STOP_MIN_DURATION_S: f64 = 3.0;
/// Speed (m/s) above which an inferred stop has ended.
pub const PIT_STOP_END_SPEED_MPS: f64 = 2.0;
/// Raw `mPitState` 5: observed, not documented (A-LMU-063) — in the garage stall and while driving from
/// the garage down the pit lane. Matched by raw value so the engine does not depend on how the model names it.
const LMU_PIT_STATE_GARAGE_RAW: u8 = 5;

/// Telemetry-rate inputs of the race engine.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RaceSample {
    /// LMU elapsed time (s).
    pub et_s: f64,
    /// Vehicle speed (m/s).
    pub speed_mps: Option<f64>,
    /// Pit-lane bit of the telemetry sector.
    pub in_pit_lane: Option<bool>,
    /// Progress through the current lap, 0..1.
    pub lap_progress: Option<f64>,
}

impl RaceSample {
    /// From a canonical frame; `None` when the sample time is unknown.
    pub fn from_frame(frame: &TelemetryFrame) -> Option<Self> {
        let et_s = finite(frame.source_et_s).or_else(|| frame_f64(frame, ch::ELAPSED_TIME))?;
        Some(Self {
            et_s,
            speed_mps: frame_f64(frame, ch::SPEED),
            in_pit_lane: frame_bool(frame, ch::IN_PIT_LANE),
            lap_progress: frame_f64(frame, ch::LAP_PROGRESS),
        })
    }
}

/// Where the car is relative to the pits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PitPhase {
    /// On the circuit.
    #[default]
    OnTrack,
    /// In the pit lane, before the stop.
    Entering,
    /// Stopped in the pit box.
    Stopped,
    /// In the pit lane after the stop (or a drive-through).
    Exiting,
    /// In the garage stall.
    Garage,
}

/// Which signal decided a pit phase or event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PitSignalSource {
    /// LMU scoring pit state (`mPitState`).
    LmuPitState,
    /// LMU pit-lane flag (telemetry sector sign bit or `mInPits`).
    LmuPitLaneFlag,
    /// LMU garage flag (`mInGarageStall`).
    LmuGarageFlag,
    /// Inferred from speed because no LMU signal exists ("CALCULATED", spec §66).
    SpeedInference,
}

impl PitSignalSource {
    /// Provenance of a value decided by this source.
    pub const fn provenance(self) -> Provenance {
        match self {
            PitSignalSource::SpeedInference => Provenance::Estimated,
            PitSignalSource::LmuPitState
            | PitSignalSource::LmuPitLaneFlag
            | PitSignalSource::LmuGarageFlag => Provenance::Derived,
        }
    }

    /// True for inferred values, which the UI must label `CALCULATED`.
    pub const fn is_calculated(self) -> bool {
        matches!(self, PitSignalSource::SpeedInference)
    }
}

/// A pit event.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum PitEvent {
    /// Entered the pit lane.
    Entry {
        /// Time (s).
        et_s: f64,
    },
    /// Stopped in the box.
    StopStart {
        /// Time (s).
        et_s: f64,
    },
    /// Left the box.
    StopEnd {
        /// Time (s).
        et_s: f64,
        /// Stationary time (s) when the stop start was observed.
        stationary_s: Option<f64>,
    },
    /// Left the pit lane.
    Exit {
        /// Time (s).
        et_s: f64,
        /// Time in the pit lane (s) when the entry was observed.
        lane_time_s: Option<f64>,
        /// Whether the car stopped during this visit.
        stopped: bool,
    },
}

impl PitEvent {
    /// Event time (s).
    pub const fn et_s(&self) -> f64 {
        match *self {
            PitEvent::Entry { et_s }
            | PitEvent::StopStart { et_s }
            | PitEvent::StopEnd { et_s, .. }
            | PitEvent::Exit { et_s, .. } => et_s,
        }
    }
}

/// A pit event with its source.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PitEventRecord {
    /// What happened.
    pub event: PitEvent,
    /// Which signal decided it.
    pub source: PitSignalSource,
}

impl PitEventRecord {
    /// Provenance (Estimated for speed inference).
    pub const fn provenance(&self) -> Provenance {
        self.source.provenance()
    }

    /// The matching session-log event (`None` for a stop start, logged when the stop ends).
    pub fn to_session_event(&self) -> Option<SessionEvent> {
        match self.event {
            PitEvent::Entry { .. } => Some(SessionEvent::PitEntry),
            PitEvent::Exit { .. } => Some(SessionEvent::PitExit),
            PitEvent::StopEnd { stationary_s, .. } => Some(SessionEvent::PitStop { stationary_s }),
            PitEvent::StopStart { .. } => None,
        }
    }
}

/// Inputs of the pit state machine.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PitInputs {
    /// LMU elapsed time (s).
    pub et_s: f64,
    /// Scoring pit state.
    pub pit_state: Option<PitState>,
    /// Scoring `mInPits`.
    pub in_pits: Option<bool>,
    /// Telemetry pit-lane bit.
    pub in_pit_lane: Option<bool>,
    /// Scoring garage flag.
    pub in_garage: Option<bool>,
    /// Speed (m/s).
    pub speed_mps: Option<f64>,
}

/// The last completed pit visit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct PitVisit {
    /// Pit lane entry (s).
    pub entry_et_s: Option<f64>,
    /// Stop start (s).
    pub stop_start_et_s: Option<f64>,
    /// Stop end (s).
    pub stop_end_et_s: Option<f64>,
    /// Pit lane exit (s).
    pub exit_et_s: Option<f64>,
    /// Stationary time (s).
    pub stationary_s: Option<f64>,
    /// Pit lane time (s).
    pub lane_time_s: Option<f64>,
    /// The stop was inferred from speed.
    pub stop_inferred: bool,
}

/// Pit state machine: entry, lane, stop, exit (spec §66).
#[derive(Debug, Clone, Default)]
pub struct PitStateMachine {
    phase: PitPhase,
    source: Option<PitSignalSource>,
    since_et_s: Option<f64>,
    visit: PitVisit,
    stopped_this_visit: bool,
    stationary_since_s: Option<f64>,
    last_visit: Option<PitVisit>,
    stops_observed: u32,
}

impl PitStateMachine {
    /// Current phase.
    pub fn phase(&self) -> PitPhase {
        self.phase
    }

    /// Source of the current phase.
    pub fn source(&self) -> Option<PitSignalSource> {
        self.source
    }

    /// Time the current phase started (s).
    pub fn since_et_s(&self) -> Option<f64> {
        self.since_et_s
    }

    /// The last completed pit visit.
    pub fn last_visit(&self) -> Option<PitVisit> {
        self.last_visit
    }

    /// Stops observed by the machine.
    pub fn stops_observed(&self) -> u32 {
        self.stops_observed
    }

    /// Feeds one input; returns the events of this step in order.
    pub fn update(&mut self, i: &PitInputs) -> Vec<PitEventRecord> {
        let et = i.et_s;
        match finite(i.speed_mps) {
            Some(v) if v < PIT_STOP_MAX_SPEED_MPS => {
                self.stationary_since_s.get_or_insert(et);
            }
            Some(v) if v > PIT_STOP_END_SPEED_MPS => self.stationary_since_s = None,
            _ => {}
        }
        let inferred_stop = self
            .stationary_since_s
            .is_some_and(|t| et - t >= PIT_STOP_MIN_DURATION_S);
        let Some((target, source)) = self.observe(i, inferred_stop) else {
            return Vec::new();
        };
        self.transition(target, source, et)
    }

    /// The phase the signals describe, with the deciding source.
    fn observe(&self, i: &PitInputs, inferred_stop: bool) -> Option<(PitPhase, PitSignalSource)> {
        use PitSignalSource as S;
        if i.in_garage == Some(true) {
            return Some((PitPhase::Garage, S::LmuGarageFlag));
        }
        let lane = i.in_pit_lane.or(i.in_pits);
        let lane_phase = if self.stopped_this_visit || self.phase == PitPhase::Garage {
            PitPhase::Exiting
        } else {
            PitPhase::Entering
        };
        match i.pit_state {
            Some(PitState::Entering) => Some((PitPhase::Entering, S::LmuPitState)),
            Some(PitState::Stopped) => Some((PitPhase::Stopped, S::LmuPitState)),
            Some(PitState::Exiting) => {
                if i.in_pit_lane == Some(false) && self.phase == PitPhase::Exiting {
                    // The telemetry lane bit leaves the lane before the scoring state catches up.
                    Some((PitPhase::OnTrack, S::LmuPitLaneFlag))
                } else {
                    Some((PitPhase::Exiting, S::LmuPitState))
                }
            }
            Some(ps) if ps.raw() == LMU_PIT_STATE_GARAGE_RAW => {
                if i.in_pit_lane == Some(false) && self.phase == PitPhase::Exiting {
                    Some((PitPhase::OnTrack, S::LmuPitLaneFlag))
                } else if self.phase == PitPhase::OnTrack {
                    // Joined while the car is in or leaving its garage: no pit entry happened.
                    Some((PitPhase::Garage, S::LmuPitState))
                } else {
                    Some((PitPhase::Exiting, S::LmuPitState))
                }
            }
            Some(PitState::Idle | PitState::Requested) => match lane {
                // Scoring lags the telemetry lane bit at the entry.
                Some(true) => Some((lane_phase, S::LmuPitLaneFlag)),
                _ => Some((PitPhase::OnTrack, S::LmuPitState)),
            },
            // Unknown raw values are never coerced: fall back to the lane bit.
            Some(_) | None => match lane {
                Some(true) if inferred_stop => Some((PitPhase::Stopped, S::SpeedInference)),
                Some(true) if self.phase == PitPhase::Stopped => {
                    // The inferred stop ends only when the car moves again.
                    if self.stationary_since_s.is_some() {
                        Some((PitPhase::Stopped, S::SpeedInference))
                    } else {
                        Some((PitPhase::Exiting, S::SpeedInference))
                    }
                }
                Some(true) => Some((lane_phase, S::LmuPitLaneFlag)),
                Some(false) => Some((PitPhase::OnTrack, S::LmuPitLaneFlag)),
                None => match finite(i.speed_mps) {
                    // No LMU pit signal at all: only a stationary car can be told apart.
                    Some(_) if inferred_stop => Some((PitPhase::Stopped, S::SpeedInference)),
                    Some(_)
                        if self.phase == PitPhase::Stopped && self.stationary_since_s.is_some() =>
                    {
                        Some((PitPhase::Stopped, S::SpeedInference))
                    }
                    Some(_) => Some((PitPhase::OnTrack, S::SpeedInference)),
                    None => None,
                },
            },
        }
    }

    /// Moves to `target`, emitting the events the move implies.
    fn transition(
        &mut self,
        target: PitPhase,
        source: PitSignalSource,
        et: f64,
    ) -> Vec<PitEventRecord> {
        use PitPhase as P;
        let mut events = Vec::new();
        if target == self.phase {
            return events;
        }
        let from = self.phase;
        // Backward moves inside the lane are state bounces, not events.
        if matches!(
            (from, target),
            (P::Stopped, P::Entering) | (P::Exiting, P::Entering)
        ) {
            return events;
        }
        let emit = |event: PitEvent, events: &mut Vec<PitEventRecord>| {
            events.push(PitEventRecord { event, source });
        };
        match (from, target) {
            (P::OnTrack, P::Entering | P::Stopped | P::Exiting) => {
                self.visit = PitVisit {
                    entry_et_s: Some(et),
                    ..PitVisit::default()
                };
                self.stopped_this_visit = false;
                emit(PitEvent::Entry { et_s: et }, &mut events);
                if target == P::Stopped {
                    self.start_stop(source, et, &mut events);
                }
            }
            (P::Entering | P::Exiting, P::Stopped) => self.start_stop(source, et, &mut events),
            (P::Stopped, P::Exiting) => self.end_stop(source, et, &mut events),
            (P::Stopped, P::OnTrack) => {
                self.end_stop(source, et, &mut events);
                self.exit(source, et, &mut events);
            }
            (P::Entering | P::Exiting, P::OnTrack) => self.exit(source, et, &mut events),
            (P::Garage, P::Entering | P::Exiting | P::Stopped) => {
                // Leaving the garage into the pit lane: the visit starts here, without an entry.
                self.visit = PitVisit::default();
                self.stopped_this_visit = false;
            }
            (P::Stopped, P::Garage) => self.end_stop(source, et, &mut events),
            _ => {}
        }
        debug!(component = COMPONENT, ?from, to = ?target, ?source, "pit phase");
        self.phase = if from == P::Garage && target == P::Entering {
            P::Exiting
        } else {
            target
        };
        self.source = Some(source);
        self.since_et_s = Some(et);
        events
    }

    fn start_stop(&mut self, source: PitSignalSource, et: f64, events: &mut Vec<PitEventRecord>) {
        // An inferred stop started when the car became stationary.
        let start = if source.is_calculated() {
            self.stationary_since_s.unwrap_or(et)
        } else {
            et
        };
        self.visit.stop_start_et_s = Some(start);
        self.visit.stop_inferred = source.is_calculated();
        self.stopped_this_visit = true;
        events.push(PitEventRecord {
            event: PitEvent::StopStart { et_s: start },
            source,
        });
    }

    fn end_stop(&mut self, source: PitSignalSource, et: f64, events: &mut Vec<PitEventRecord>) {
        let stationary = self
            .visit
            .stop_start_et_s
            .map(|s| et - s)
            .filter(|d| *d >= 0.0);
        self.visit.stop_end_et_s = Some(et);
        self.visit.stationary_s = stationary;
        self.stops_observed = self.stops_observed.saturating_add(1);
        events.push(PitEventRecord {
            event: PitEvent::StopEnd {
                et_s: et,
                stationary_s: stationary,
            },
            source,
        });
    }

    fn exit(&mut self, source: PitSignalSource, et: f64, events: &mut Vec<PitEventRecord>) {
        let lane = self.visit.entry_et_s.map(|e| et - e).filter(|d| *d >= 0.0);
        self.visit.exit_et_s = Some(et);
        self.visit.lane_time_s = lane;
        self.last_visit = Some(self.visit);
        events.push(PitEventRecord {
            event: PitEvent::Exit {
                et_s: et,
                lane_time_s: lane,
                stopped: self.stopped_this_visit,
            },
            source,
        });
        self.stopped_this_visit = false;
    }
}

/// A gap to another car.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Gap {
    /// Gap in seconds (when on the same lap or reported by LMU).
    pub seconds: Option<f64>,
    /// Whole laps between the cars (0 = same lap).
    pub laps: Option<i32>,
    /// Slot id of the other car.
    pub other_slot_id: Option<i32>,
    /// Measured (LMU) or Estimated (difference of times behind the leader).
    pub provenance: Provenance,
    /// Formula id or LMU origin.
    pub formula: &'static str,
}

impl Gap {
    fn none(provenance: Provenance, formula: &'static str) -> Self {
        Self {
            seconds: None,
            laps: None,
            other_slot_id: None,
            provenance,
            formula,
        }
    }
}

/// Race-control state (spec §35). `None` = not reported. LMU does not distinguish a safety car from a
/// full-course yellow, so both are `full_course_yellow`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct FlagState {
    /// Game phase.
    pub game_phase: Option<GamePhase>,
    /// Full-course yellow state.
    pub yellow_flag_state: Option<YellowFlagState>,
    /// Green-flag running.
    pub green: Option<bool>,
    /// Full-course yellow or safety car (`mGamePhase == 6` or yellow state 1..=6).
    pub full_course_yellow: Option<bool>,
    /// Pits closed during a full-course yellow.
    pub pits_closed: Option<bool>,
    /// Blue flag for the player.
    pub blue: Option<bool>,
    /// The player is in a local yellow zone (`mUnderYellow`).
    pub local_yellow: Option<bool>,
    /// Session stopped or race halted.
    pub red: Option<bool>,
    /// Session over or the player has finished.
    pub chequered: Option<bool>,
    /// Formation lap.
    pub formation: Option<bool>,
    /// Raw sector flags; their sector order is unverified (A-LMU-030).
    pub sector_flags_raw: Option<[i8; 3]>,
}

impl FlagState {
    /// Most important flag for the summary display: red, chequered, full-course yellow, blue, local
    /// yellow, green.
    pub fn summary(&self) -> RaceFlag {
        let on = |v: Option<bool>| v == Some(true);
        if on(self.red) {
            RaceFlag::Red
        } else if on(self.chequered) {
            RaceFlag::Chequered
        } else if on(self.full_course_yellow) {
            RaceFlag::FullCourseYellow
        } else if on(self.blue) {
            RaceFlag::Blue
        } else if on(self.local_yellow) {
            RaceFlag::Yellow
        } else if on(self.green) {
            RaceFlag::Green
        } else {
            RaceFlag::None
        }
    }
}

/// Conditions seen during a lap (fuel exclusion inputs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LapConditions {
    /// Formation at any time during the lap.
    pub formation: bool,
    /// Full-course yellow / safety car at any time during the lap.
    pub caution: bool,
}

/// Pit status for display.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PitStatus {
    /// Current phase.
    pub phase: PitPhase,
    /// Source of the phase (`SpeedInference` = CALCULATED).
    pub source: Option<PitSignalSource>,
    /// Phase start (s).
    pub since_et_s: Option<f64>,
    /// Last completed visit.
    pub last_visit: Option<PitVisit>,
    /// Stops observed by the engine.
    pub stops_observed: u32,
    /// LMU pit stop count (`mNumPitstops`).
    pub lmu_pit_stops: Metric<i32>,
}

/// Race state for display and publication.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RaceSnapshot {
    /// Player slot id.
    pub slot_id: Option<i32>,
    /// Overall position.
    pub position: Metric<u8>,
    /// Position in class.
    pub class_position: Metric<u8>,
    /// Class name.
    pub class_name: Option<String>,
    /// Driver name.
    pub driver: Option<String>,
    /// Team (pit group).
    pub team: Option<String>,
    /// Vehicle name.
    pub vehicle: Option<String>,
    /// Session type.
    pub session_type: Option<SessionType>,
    /// Completed laps (`mTotalLaps`).
    pub laps_completed: Metric<i32>,
    /// Scheduled laps; `None` for a timed session.
    pub total_laps: Option<u32>,
    /// Race laps remaining for the player.
    pub race_laps_remaining: Metric<f64>,
    /// How the race ends.
    pub race_finish: RaceFinish,
    /// Session elapsed time (s).
    pub session_elapsed_s: Metric<f64>,
    /// Session time remaining (s).
    pub session_remaining_s: Metric<f64>,
    /// Gap to the car ahead overall.
    pub gap_ahead: Gap,
    /// Gap to the car behind overall.
    pub gap_behind: Gap,
    /// Gap to the class car ahead.
    pub class_gap_ahead: Gap,
    /// Gap to the class car behind.
    pub class_gap_behind: Gap,
    /// Pit status.
    pub pit: PitStatus,
    /// Flags.
    pub flags: FlagState,
    /// Slot ids are unique and at most one player row exists.
    pub standings_consistent: bool,
}

/// What happened on one update.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RaceUpdate {
    /// Pit events, in order.
    pub pit_events: Vec<PitEventRecord>,
    /// Summary flag change `(from, to)`.
    pub flag_change: Option<(RaceFlag, RaceFlag)>,
}

/// Race state engine. Deterministic and clock-free.
#[derive(Debug, Clone, Default)]
pub struct RaceEngine {
    pit: PitStateMachine,
    lap_conditions: LapConditions,
    last_flag: Option<RaceFlag>,
    reference_pace_s: Option<f64>,
    snapshot: Option<RaceSnapshot>,
}

impl RaceEngine {
    /// A new engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pace (s/lap) used for the laps remaining of a timed race, e.g. the stint average; when `None` the
    /// LMU estimated lap time is used.
    pub fn set_reference_pace(&mut self, pace_s: Option<f64>) {
        self.reference_pace_s = finite(pace_s).filter(|p| *p > 0.0);
    }

    /// The pit state machine.
    pub fn pit(&self) -> &PitStateMachine {
        &self.pit
    }

    /// Latest snapshot (`None` before the first update).
    pub fn snapshot(&self) -> Option<&RaceSnapshot> {
        self.snapshot.as_ref()
    }

    /// Conditions of the lap in progress so far.
    pub fn lap_conditions(&self) -> LapConditions {
        self.lap_conditions
    }

    /// Returns the conditions of the lap that just completed and starts the next lap with the conditions
    /// in force now (a caution running over the line also marks the new lap).
    pub fn take_lap_conditions(&mut self) -> LapConditions {
        let done = self.lap_conditions;
        let flags = self.snapshot.as_ref().map(|s| s.flags).unwrap_or_default();
        self.lap_conditions = LapConditions {
            formation: flags.formation == Some(true),
            caution: flags.full_course_yellow == Some(true),
        };
        done
    }

    /// Feeds the latest scoring state and a telemetry-rate sample.
    pub fn update(&mut self, scoring: &ScoringState, sample: &RaceSample) -> RaceUpdate {
        let player = scoring.player();
        let session = &scoring.session;
        let pit_events = self.pit.update(&PitInputs {
            et_s: sample.et_s,
            pit_state: player.and_then(|p| p.pit_state),
            in_pits: player.and_then(|p| p.in_pits),
            in_pit_lane: sample.in_pit_lane,
            in_garage: player.and_then(|p| p.in_garage_stall),
            speed_mps: sample.speed_mps,
        });

        let flags = flag_state(scoring, player);
        if flags.formation == Some(true) {
            self.lap_conditions.formation = true;
        }
        if flags.full_course_yellow == Some(true) {
            self.lap_conditions.caution = true;
        }
        let summary = flags.summary();
        let flag_change = match self.last_flag {
            Some(prev) if prev != summary => Some((prev, summary)),
            _ => None,
        };
        self.last_flag = Some(summary);

        let progress = finite(sample.lap_progress).map(|p| p.clamp(0.0, 1.0));
        let completed = player.and_then(|p| p.laps).filter(|l| *l >= 0);
        let race_finish = match (session.max_laps, completed) {
            (Some(total), Some(done)) => {
                let done = u32::try_from(done).unwrap_or(0);
                // Unknown progress counts the current lap fully (conservative for fuel).
                RaceFinish::lap_race(total, done, progress.unwrap_or(0.0))
            }
            (Some(_), None) => RaceFinish::Unknown,
            (None, _) => match remaining_time(session) {
                Some(t) => RaceFinish::Time {
                    time_remaining_s: t,
                },
                None => RaceFinish::Unknown,
            },
        };
        let pace = self
            .reference_pace_s
            .or_else(|| finite(player.and_then(|p| p.estimated_lap_s)).filter(|p| *p > 0.0));
        let race_laps_remaining = race_finish.laps_to_finish(progress, pace);

        let class_ranks = class_ranking(scoring);
        let class_position = player.and_then(|p| {
            p.class_place.or_else(|| {
                class_ranks
                    .iter()
                    .find(|(id, _)| *id == p.slot_id)
                    .map(|r| r.1)
            })
        });

        let snapshot = RaceSnapshot {
            slot_id: player.map(|p| p.slot_id),
            position: Metric::measured("scoring.mPlace", player.and_then(|p| p.place)),
            class_position: Metric::derived(formula::CLASS_POSITION, class_position),
            class_name: player.and_then(|p| p.class_name.clone()),
            driver: player.and_then(|p| p.driver.clone()),
            team: player.and_then(|p| p.pit_group.clone()),
            vehicle: player.and_then(|p| p.vehicle_name.clone()),
            session_type: session.session_type,
            laps_completed: Metric::measured("scoring.mTotalLaps", completed),
            total_laps: session.max_laps,
            race_laps_remaining: Metric::estimated(
                formula::RACE_LAPS_REMAINING,
                race_laps_remaining,
            ),
            race_finish,
            session_elapsed_s: Metric::measured("scoring.mCurrentET", finite(session.current_et_s)),
            session_remaining_s: match finite(session.time_remaining_s) {
                Some(t) => Metric::measured("scoring.mSessionTimeRemaining", Some(t)),
                None => Metric::derived(
                    "session_time_remaining_end_minus_current",
                    session.time_remaining_calc_s(),
                ),
            },
            gap_ahead: overall_gap_ahead(player),
            gap_behind: overall_gap_behind(scoring, player),
            class_gap_ahead: class_gap(scoring, player, &class_ranks, class_position, true),
            class_gap_behind: class_gap(scoring, player, &class_ranks, class_position, false),
            pit: PitStatus {
                phase: self.pit.phase(),
                source: self.pit.source(),
                since_et_s: self.pit.since_et_s(),
                last_visit: self.pit.last_visit(),
                stops_observed: self.pit.stops_observed(),
                lmu_pit_stops: Metric::measured(
                    "scoring.mNumPitstops",
                    player.and_then(|p| p.num_pitstops),
                ),
            },
            flags,
            standings_consistent: scoring.is_consistent(),
        };
        self.snapshot = Some(snapshot);
        RaceUpdate {
            pit_events,
            flag_change,
        }
    }
}

/// Session time remaining: the LMU value, else `mEndET − mCurrentET`.
fn remaining_time(session: &telemetry_model::scoring::SessionScoring) -> Option<f64> {
    finite(session.time_remaining_s).or_else(|| {
        lmu_session_time_remaining(finite(session.end_et_s)?, finite(session.current_et_s)?)
    })
}

/// Flags from the session and the player row.
fn flag_state(scoring: &ScoringState, player: Option<&CompetitorEntry>) -> FlagState {
    let s = &scoring.session;
    let phase = s.game_phase;
    let yfs = s.yellow_flag_state;
    let fcy = match (phase, yfs) {
        (None, None) => None,
        _ => Some(
            phase == Some(GamePhase::FullCourseYellow)
                || yfs.is_some_and(|y| (1..=6).contains(&y.raw())),
        ),
    };
    let red = match (phase, yfs) {
        (None, None) => None,
        _ => {
            Some(phase == Some(GamePhase::SessionStopped) || yfs == Some(YellowFlagState::RaceHalt))
        }
    };
    let finished = player
        .and_then(|p| p.finish_status)
        .map(|f| f == FinishStatus::Finished);
    let chequered = match (phase, finished) {
        (None, None) => None,
        _ => Some(phase == Some(GamePhase::SessionOver) || finished == Some(true)),
    };
    let formation = match (phase, player.and_then(|p| p.individual_phase)) {
        (None, None) => None,
        (p, ip) => Some(p == Some(GamePhase::Formation) || ip == Some(IndividualPhase::Formation)),
    };
    FlagState {
        game_phase: phase,
        yellow_flag_state: yfs,
        green: phase.map(|p| p == GamePhase::GreenFlag && fcy != Some(true)),
        full_course_yellow: fcy,
        pits_closed: yfs.map(|y| y == YellowFlagState::PitsClosed),
        blue: player.and_then(|p| p.flag).map(|f| f == CarFlag::Blue),
        local_yellow: player.and_then(|p| p.under_yellow),
        red,
        chequered,
        formation,
        sector_flags_raw: s.sector_flags_raw,
    }
}

/// `(slot id, class position)` for every row with a class and a place.
fn class_ranking(scoring: &ScoringState) -> Vec<(i32, u8)> {
    let mut rows: Vec<&CompetitorEntry> = scoring
        .vehicles
        .iter()
        .filter(|v| v.class_name.is_some() && v.place.is_some())
        .collect();
    rows.sort_by_key(|v| (v.place, v.slot_id));
    let mut counters: std::collections::BTreeMap<&str, u8> = std::collections::BTreeMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for v in rows {
        let Some(class) = v.class_name.as_deref() else {
            continue;
        };
        let n = counters.entry(class).or_insert(0);
        *n = n.saturating_add(1);
        out.push((v.slot_id, *n));
    }
    out
}

fn overall_gap_ahead(player: Option<&CompetitorEntry>) -> Gap {
    let origin = "scoring.mTimeBehindNext";
    match player {
        Some(p) if p.place.is_some_and(|pl| pl > 1) => Gap {
            seconds: finite(p.time_behind_next_s).filter(|t| *t >= 0.0),
            laps: p.laps_behind_next.filter(|l| *l >= 0),
            other_slot_id: None,
            provenance: Provenance::Measured,
            formula: origin,
        },
        _ => Gap::none(Provenance::Measured, origin),
    }
}

fn overall_gap_behind(scoring: &ScoringState, player: Option<&CompetitorEntry>) -> Gap {
    let origin = "scoring.mTimeBehindNext (car behind)";
    let behind = player
        .and_then(|p| p.place)
        .and_then(|pl| pl.checked_add(1))
        .and_then(|next| scoring.vehicles.iter().find(|v| v.place == Some(next)));
    match behind {
        Some(b) => Gap {
            seconds: finite(b.time_behind_next_s).filter(|t| *t >= 0.0),
            laps: b.laps_behind_next.filter(|l| *l >= 0),
            other_slot_id: Some(b.slot_id),
            provenance: Provenance::Measured,
            formula: origin,
        },
        None => Gap::none(Provenance::Measured, origin),
    }
}

/// Gap to the class car directly ahead (`ahead`) or behind, from the times behind the leader.
fn class_gap(
    scoring: &ScoringState,
    player: Option<&CompetitorEntry>,
    ranks: &[(i32, u8)],
    class_position: Option<u8>,
    ahead: bool,
) -> Gap {
    let none = Gap::none(Provenance::Estimated, formula::GAP_FROM_LEADER_TIMES);
    let (Some(p), Some(cp)) = (player, class_position) else {
        return none;
    };
    let target = if ahead {
        cp.checked_sub(1)
    } else {
        cp.checked_add(1)
    };
    let Some(target) = target.filter(|t| *t >= 1) else {
        return none;
    };
    let other = ranks
        .iter()
        .filter(|(id, rank)| *rank == target && *id != p.slot_id)
        .filter_map(|(id, _)| scoring.by_slot(*id))
        .find(|v| v.class_name == p.class_name);
    let Some(o) = other else {
        return none;
    };
    // Positive gap = the car behind is that far behind the car ahead.
    let (front, back) = if ahead { (o, p) } else { (p, o) };
    let laps = back
        .laps_behind_leader
        .zip(front.laps_behind_leader)
        .map(|(b, f)| b - f);
    let seconds = match laps {
        Some(0) => finite(back.time_behind_leader_s)
            .zip(finite(front.time_behind_leader_s))
            .map(|(b, f)| b - f),
        _ => None,
    };
    Gap {
        seconds,
        laps,
        other_slot_id: Some(o.slot_id),
        provenance: Provenance::Estimated,
        formula: formula::GAP_FROM_LEADER_TIMES,
    }
}

#[cfg(test)]
mod tests;

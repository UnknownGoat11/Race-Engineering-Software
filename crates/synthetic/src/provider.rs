//! [`SyntheticTelemetryProvider`]: the seeded, deterministic generator that turns a [`Scenario`] into a
//! stream of [`SyntheticTick`]s — a [`TelemetryFrame`] at the telemetry rate (default 100 Hz) and a
//! [`ScoringSnapshot`] at the scoring rate (default 5 Hz).
//!
//! **DEV / TEST / DEMO ONLY.** Construction requires a [`SyntheticMode`] token, every tick carries
//! [`DataOrigin::Synthetic`], and every identity string in frames and scoring carries the synthetic prefix.
//!
//! Time model: tick `n` of the session describes the state after `n` simulation steps of `1 / rate`
//! seconds, at session time `n / rate` (computed from the tick count, never accumulated, so it does not
//! drift). The provider's monotonic clock counts every tick, including the ticks of a source stall, during
//! which session time is frozen.
//!
//! Determinism: the same scenario (including its seed) always produces bit-identical ticks. Independent
//! ChaCha streams drive the simulation, the weather noise, the field strategy and the malformed-value
//! injection, so enabling injection does not change any clean value.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::scoring::SessionIdentity;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::car::{Car, CompletedLap, PitPhase, SpeedContext, StepOutcome};
use crate::dynamics::{self, Dynamics, ServiceRequest, TickInput, gaussian};
use crate::error::SyntheticError;
use crate::inject::{self, Injection};
use crate::mode::{DataOrigin, SyntheticMode, synthetic_name};
use crate::profile::SpeedProfile;
use crate::scenario::{Scenario, StaleMode};
use crate::session::{self, CarIdentity, FlagState, ScoringInputs, ScoringSnapshot};
use crate::track::Track;
use crate::vehicle::VehicleProfile;
use crate::weather::WeatherState;
use crate::writer::{self, FrameContext};

/// Grip multiplier of a fully wet track (dry = 1); also the grip of the wet speed profile.
pub const WET_GRIP: f64 = 0.72;
/// Default queue capacity of [`SyntheticTelemetryProvider::into_realtime`] (ticks).
pub const DEFAULT_REALTIME_QUEUE: usize = 256;
/// Rolling start: the pole sitter starts this far before the line, the others `GRID_SPACING_M` apart (m).
const GRID_FIRST_M: f64 = 5.0;
const GRID_SPACING_M: f64 = 25.0;
/// Base pace spread across the field, first to last grid place (fraction of the lap time).
const FIELD_PACE_SPREAD: f64 = 0.015;
/// Per-car random base pace offset (1 σ, fraction).
const CAR_PACE_SIGMA: f64 = 0.003;
/// Every pace multiplier stays inside this range (1.0 = the ideal profile; larger = slower).
const PACE_RANGE: (f64, f64) = (0.97, 1.08);
/// Competitor stationary service time range (s).
const COMPETITOR_SERVICE_S: (f64, f64) = (24.0, 34.0);
/// Laps of energy competitors keep in reserve when planning a stint.
const STINT_RESERVE_LAPS: f64 = 1.0;
/// Competitor stops are staggered over this many laps.
const STINT_STAGGER_LAPS: u32 = 3;
/// Resolution of the delta-to-best reference trace (m).
const DELTA_BUCKET_M: f64 = 10.0;
/// A scheduled track-limits excursion starts at this fraction of the lap.
const EXCURSION_LAP_FRACTION: f64 = 0.55;
/// The monotonic clock starts here so that capture times are never zero (ns).
const MONO_START_NS: u64 = 1_000_000_000;
/// Nanoseconds per second.
const NANOS_PER_SECOND: f64 = 1e9;
/// Guards the scoring-epoch floor against representation error of exact multiples.
const EPOCH_EPSILON: f64 = 1e-9;
/// ChaCha stream ids of the independent random sources.
const STREAM_SIM: u64 = 0;
const STREAM_WEATHER: u64 = 1;
const STREAM_INJECT: u64 = 2;
const STREAM_FIELD: u64 = 3;

/// Something notable that happened during a tick (for tests, demos and logs).
#[derive(Debug, Clone, PartialEq)]
pub enum SyntheticEvent {
    /// A car completed a lap.
    LapCompleted {
        slot_id: i32,
        is_player: bool,
        lap: CompletedLap,
    },
    /// A car crossed the pit entry line.
    PitLaneEntered { slot_id: i32 },
    /// A car stopped in its box; `service_s` is the stationary time.
    PitBoxStop { slot_id: i32, service_s: f64 },
    /// A car crossed the pit exit line.
    PitLaneExited { slot_id: i32 },
    /// The player ran wide; `lap` (1-based) is invalidated.
    TrackLimits { lap: u32 },
    /// The source stalled.
    StallStarted { mode: StaleMode },
    /// The source resumed.
    StallEnded,
}

/// One provider step.
#[derive(Debug, Clone, PartialEq)]
pub struct SyntheticTick {
    /// Always [`DataOrigin::Synthetic`]: consumers must label the data.
    pub origin: DataOrigin,
    /// Provider tick index (counts stalled ticks too).
    pub tick: u64,
    /// Monotonic time of the tick (ns), equal to the frame's `capture_mono_ns`: simulated
    /// (1 s + tick × interval) from [`SyntheticTelemetryProvider::next_tick`], the real emission time in a
    /// [`RealtimeStream`].
    pub capture_mono_ns: u64,
    /// Session time (s); frozen during a stall.
    pub session_et_s: f64,
    /// The telemetry frame; `None` while the source delivers nothing ([`StaleMode::NoFrames`]).
    pub frame: Option<TelemetryFrame>,
    /// A scoring update, at the scoring rate.
    pub scoring: Option<ScoringSnapshot>,
    /// The malformed value injected into this frame, if any.
    pub injection: Option<Injection>,
    /// Set while the source is stalled.
    pub stale: Option<StaleMode>,
    pub events: Vec<SyntheticEvent>,
}

/// A competitor's stint plan (unused for the player).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Strategy {
    base_pace: f64,
    stint_laps: u32,
    next_stop_lap: u32,
    stint_start_progress_m: f64,
}

/// A stall in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveStall {
    mode: StaleMode,
    remaining_ticks: u64,
}

/// Reference trace of the best lap for the delta-to-best channel: elapsed lap time at every
/// [`DELTA_BUCKET_M`] boundary, interpolated to the exact boundary crossing.
#[derive(Debug, Clone, PartialEq)]
struct LapTrace {
    length_m: f64,
    current: Vec<Option<f64>>,
    /// Elapsed time at every bucket start of the best lap, then its lap time (at the track length).
    best: Option<Vec<f64>>,
    last_index: Option<usize>,
    /// Lap distance and elapsed time of the last recorded sample of the current lap.
    last_sample: Option<(f64, f64)>,
}

impl LapTrace {
    fn new(track_length_m: f64) -> Self {
        // Bucket count of a track of at most a few tens of km: far inside usize.
        let buckets = (track_length_m / DELTA_BUCKET_M).ceil().max(1.0) as usize;
        Self {
            length_m: track_length_m,
            current: vec![None; buckets],
            best: None,
            last_index: None,
            last_sample: None,
        }
    }

    /// Starts a new lap at the line (elapsed 0 at bucket 0).
    fn start_lap(&mut self) {
        self.current.iter_mut().for_each(|b| *b = None);
        if let Some(first) = self.current.first_mut() {
            *first = Some(0.0);
        }
        self.last_index = Some(0);
        self.last_sample = Some((0.0, 0.0));
    }

    /// Records the buckets crossed during the last step, which ended at `s_m` with `elapsed_s` after
    /// driving `step_m` in `dt_s`.
    fn record(&mut self, s_m: f64, elapsed_s: f64, step_m: f64, dt_s: f64) {
        let Some(last) = self.last_index else {
            return;
        };
        self.last_sample = Some((s_m, elapsed_s));
        let index = ((s_m / DELTA_BUCKET_M).floor() as usize).min(self.current.len() - 1);
        if index <= last {
            return;
        }
        for j in last + 1..=index {
            let behind_m = s_m - j as f64 * DELTA_BUCKET_M;
            let fraction = if step_m > 0.0 {
                (behind_m / step_m).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if let Some(slot) = self.current.get_mut(j) {
                *slot = Some(elapsed_s - fraction * dt_s);
            }
        }
        self.last_index = Some(index);
    }

    /// Fills the buckets between the last recorded sample and the line, which the line-crossing step
    /// passed before it was recorded, by interpolating to (track length, `lap_time_s`).
    fn fill_to_line(&mut self, lap_time_s: f64) {
        let (Some(last), Some((s_last, e_last))) = (self.last_index, self.last_sample) else {
            return;
        };
        let span_m = self.length_m - s_last;
        for j in last + 1..self.current.len() {
            let fraction = if span_m > 0.0 {
                ((j as f64 * DELTA_BUCKET_M - s_last) / span_m).clamp(0.0, 1.0)
            } else {
                1.0
            };
            if let Some(slot) = self.current.get_mut(j) {
                *slot = Some(e_last + (lap_time_s - e_last) * fraction);
            }
        }
    }

    /// Ends the lap; keeps it as the reference if it is the new best valid lap. A new best lap whose trace
    /// is incomplete clears the reference: an older, slower lap is never kept as the reference.
    fn complete(&mut self, lap: &CompletedLap, new_best: bool) {
        self.fill_to_line(lap.lap_time_s);
        if new_best && lap.valid {
            let complete: Option<Vec<f64>> = self.current.iter().copied().collect();
            self.best = complete.map(|mut trace| {
                trace.push(lap.lap_time_s);
                trace
            });
            if self.best.is_none() {
                tracing::warn!(
                    component = "synthetic",
                    lap = lap.lap,
                    "best-lap reference trace incomplete; delta-to-best unavailable"
                );
            }
        }
        self.start_lap();
    }

    /// Elapsed time minus the best lap's elapsed time at the same lap distance.
    fn delta(&self, s_m: f64, elapsed_s: f64) -> Option<f64> {
        let best = self.best.as_ref()?;
        let i = ((s_m / DELTA_BUCKET_M).floor() as usize).min(best.len().saturating_sub(2));
        let a = *best.get(i)?;
        let b = *best.get(i + 1)?;
        // The last segment ends at the track length, not at a full bucket.
        let start_m = i as f64 * DELTA_BUCKET_M;
        let end_m = ((i + 1) as f64 * DELTA_BUCKET_M).min(self.length_m);
        let fraction = if end_m > start_m {
            ((s_m - start_m) / (end_m - start_m)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        Some(elapsed_s - (a + (b - a) * fraction))
    }
}

/// The synthetic telemetry source. See the module documentation.
#[derive(Debug, Clone)]
pub struct SyntheticTelemetryProvider {
    mode: SyntheticMode,
    scenario: Scenario,
    track: Track,
    vehicle: VehicleProfile,
    dry: SpeedProfile,
    wet: SpeedProfile,
    cars: Vec<Car>,
    identities: Vec<CarIdentity>,
    strategies: Vec<Strategy>,
    player: usize,
    dynamics: Dynamics,
    weather: WeatherState,
    flags: FlagState,
    sim_rng: ChaCha8Rng,
    weather_rng: ChaCha8Rng,
    inject_rng: ChaCha8Rng,
    field_rng: ChaCha8Rng,
    dt_s: f64,
    session_ticks: u64,
    wall_ticks: u64,
    sequence: u64,
    scoring: ScoringSnapshot,
    scoring_epoch: Option<u64>,
    next_pit_stop: usize,
    pending_service: Option<ServiceRequest>,
    excursions_done: Vec<u32>,
    stalls_done: Vec<bool>,
    active_stall: Option<ActiveStall>,
    keep_last_frame: bool,
    last_frame: Option<(TelemetryFrame, Option<Injection>)>,
    lap_trace: LapTrace,
}

fn seeded(seed: u64, stream: u64) -> ChaCha8Rng {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    rng.set_stream(stream);
    rng
}

fn clamp_pace(pace: f64) -> f64 {
    pace.clamp(PACE_RANGE.0, PACE_RANGE.1)
}

impl SyntheticTelemetryProvider {
    /// Builds the provider. `mode` must come from [`crate::DemoMode::acknowledge`] or
    /// [`crate::TestMode::acknowledge`]; there is no other way to construct one.
    pub fn new(mode: impl Into<SyntheticMode>, scenario: Scenario) -> Result<Self, SyntheticError> {
        let mode = mode.into();
        scenario.validate()?;
        let track = Track::build(&scenario.track)?;
        let vehicle = VehicleProfile::new(scenario.vehicle);
        let dry = SpeedProfile::compute(&track, &vehicle, 1.0);
        let wet = SpeedProfile::compute(&track, &vehicle, WET_GRIP);
        let dt_s = 1.0 / scenario.telemetry_rate_hz;
        let sim_rng = seeded(scenario.seed, STREAM_SIM);
        let weather_rng = seeded(scenario.seed, STREAM_WEATHER);
        let inject_rng = seeded(scenario.seed, STREAM_INJECT);
        let mut field_rng = seeded(scenario.seed, STREAM_FIELD);

        let field = scenario.competitors + 1;
        let player = usize::from(scenario.player_start_place).saturating_sub(1);
        let length = track.length_m();
        let energy_per_lap = energy_per_lap_frac(&vehicle);
        // Whole laps per stint (small positive number), at least one.
        let stint_laps = ((1.0 / energy_per_lap - STINT_RESERVE_LAPS).floor().max(1.0)) as u32;
        let mut cars = Vec::with_capacity(field);
        let mut identities = Vec::with_capacity(field);
        let mut strategies = Vec::with_capacity(field);
        for index in 0..field {
            let is_player = index == player;
            let grid_m = GRID_FIRST_M + index as f64 * GRID_SPACING_M;
            let s_m = track.wrap(length - grid_m);
            let spread = FIELD_PACE_SPREAD * (index as f64 / field.max(2) as f64 - 0.5);
            let base_pace = if is_player {
                1.0
            } else {
                clamp_pace(1.0 + spread + CAR_PACE_SIGMA * gaussian(&mut field_rng))
            };
            // Field size ≤ MAX_COMPETITORS + 1, far inside i32.
            let slot_id = index as i32;
            let car = Car::new(
                slot_id,
                is_player,
                s_m,
                dry.speed_at(s_m) / base_pace,
                base_pace,
            );
            strategies.push(Strategy {
                base_pace,
                stint_laps,
                next_stop_lap: if is_player {
                    u32::MAX
                } else {
                    stint_laps
                        .saturating_sub(u32::try_from(index).unwrap_or(0) % STINT_STAGGER_LAPS)
                        .max(1)
                },
                stint_start_progress_m: car.progress_m(length),
            });
            identities.push(identity(&vehicle, index, is_player));
            cars.push(car);
        }

        let weather = WeatherState::new(&scenario.weather, scenario.session.duration_s);
        let lap_weight = dynamics::lap_consumption_weight(&track, &vehicle, &dry);
        let dynamics = Dynamics::new(&vehicle, &scenario.start, lap_weight, weather.air_temp_c);
        let flags = session::flags_at(&scenario.cautions, 0.0, false);
        let keep_last_frame = scenario
            .stale
            .iter()
            .any(|s| s.mode == StaleMode::FrozenFrames);
        let stalls_done = vec![false; scenario.stale.len()];
        let lap_trace = LapTrace::new(length);
        let placeholder = ScoringSnapshot {
            state: Default::default(),
            player_index: player,
            gaps: Default::default(),
            et_s: 0.0,
        };
        let mut provider = Self {
            mode,
            scenario,
            track,
            vehicle,
            dry,
            wet,
            cars,
            identities,
            strategies,
            player,
            dynamics,
            weather,
            flags,
            sim_rng,
            weather_rng,
            inject_rng,
            field_rng,
            dt_s,
            session_ticks: 0,
            wall_ticks: 0,
            sequence: 0,
            scoring: placeholder,
            scoring_epoch: None,
            next_pit_stop: 0,
            pending_service: None,
            excursions_done: Vec::new(),
            stalls_done,
            active_stall: None,
            keep_last_frame,
            last_frame: None,
            lap_trace,
        };
        // Settle pedals, gear and engine for the rolling start without consuming fuel or energy.
        let (fuel, ve) = (provider.dynamics.fuel_l, provider.dynamics.virtual_energy);
        let input = provider.tick_input(0.0, 1.0);
        provider
            .dynamics
            .update(&provider.vehicle, &input, &mut provider.sim_rng);
        provider.dynamics.fuel_l = fuel;
        provider.dynamics.virtual_energy = ve;
        provider.scoring = provider.build_scoring(0.0);
        tracing::info!(
            component = "synthetic",
            scenario = %provider.scenario.name,
            seed = provider.scenario.seed,
            purpose = ?provider.mode.purpose(),
            "synthetic telemetry provider created (SYNTHETIC DATA)"
        );
        Ok(provider)
    }

    /// Origin label of everything this provider produces.
    pub fn origin(&self) -> DataOrigin {
        DataOrigin::Synthetic(self.mode.purpose())
    }

    /// The scenario being played.
    pub fn scenario(&self) -> &Scenario {
        &self.scenario
    }

    /// The circuit.
    pub fn track(&self) -> &Track {
        &self.track
    }

    /// The player's vehicle profile.
    pub fn vehicle(&self) -> &VehicleProfile {
        &self.vehicle
    }

    /// Ideal dry lap time of the player's car on this circuit (s).
    pub fn reference_lap_time_s(&self) -> f64 {
        self.dry.lap_time_s()
    }

    /// Time between two ticks.
    pub fn tick_interval(&self) -> Duration {
        Duration::from_secs_f64(self.dt_s)
    }

    /// Session time of the last produced session tick (s).
    pub fn session_time_s(&self) -> f64 {
        self.session_ticks.saturating_sub(1) as f64 * self.dt_s
    }

    /// The latest scoring snapshot.
    pub fn latest_scoring(&self) -> &ScoringSnapshot {
        &self.scoring
    }

    /// Session identity; every name carries the synthetic prefix.
    pub fn session_identity(&self) -> SessionIdentity {
        SessionIdentity {
            vehicle_name: Some(self.vehicle.name.clone()),
            vehicle_model: Some(self.vehicle.model.clone()),
            vehicle_class: Some(self.vehicle.class),
            class_name: Some(self.vehicle.class_name.clone()),
            track_name: Some(self.track.name().to_owned()),
            session_type: Some(self.scenario.session.session_type),
            game_version: None,
        }
    }

    /// An endless iterator over [`Self::next_tick`].
    pub fn ticks(&mut self) -> impl Iterator<Item = Result<SyntheticTick, SyntheticError>> + '_ {
        std::iter::repeat_with(move || self.next_tick())
    }

    /// Produces the next tick.
    pub fn next_tick(&mut self) -> Result<SyntheticTick, SyntheticError> {
        let tick = self.wall_ticks;
        self.wall_ticks = self.wall_ticks.saturating_add(1);
        let mono = self.mono_ns(tick);
        let mut events = Vec::new();
        if self.active_stall.is_none() {
            self.maybe_start_stall(&mut events);
        }
        if let Some(mode) = self.active_stall.map(|stall| stall.mode) {
            return Ok(self.stalled_tick(tick, mono, events, mode));
        }

        let session_tick = self.session_ticks;
        self.session_ticks = self.session_ticks.saturating_add(1);
        let et = session_tick as f64 * self.dt_s;
        if session_tick > 0 {
            let et_prev = (session_tick - 1) as f64 * self.dt_s;
            self.step(et_prev, &mut events);
        }
        let session_over = et >= self.scenario.session.duration_s;
        self.flags = session::flags_at(&self.scenario.cautions, et, session_over);

        let epoch = ((session_tick as f64 * self.scenario.scoring_rate_hz
            / self.scenario.telemetry_rate_hz)
            + EPOCH_EPSILON)
            .floor() as u64;
        let scoring = if self.scoring_epoch != Some(epoch) {
            self.scoring_epoch = Some(epoch);
            self.scoring = self.build_scoring(et);
            Some(self.scoring.clone())
        } else {
            None
        };

        self.sequence = self.sequence.saturating_add(1);
        let mut frame = TelemetryFrame::new(self.sequence, mono, Some(et));
        writer::write_frame(&mut frame, &self.frame_context(et))?;
        let injection = match &self.scenario.malformed {
            Some(spec) => inject::inject(&mut frame, spec, et, &mut self.inject_rng)?,
            None => None,
        };
        if self.keep_last_frame {
            self.last_frame = Some((frame.clone(), injection));
        }
        Ok(SyntheticTick {
            origin: self.origin(),
            tick,
            capture_mono_ns: mono,
            session_et_s: et,
            frame: Some(frame),
            scoring,
            injection,
            stale: None,
            events,
        })
    }

    /// [`Self::into_realtime_with_epoch`] with a clock epoch of "now".
    pub fn into_realtime(self, queue_capacity: usize) -> Result<RealtimeStream, SyntheticError> {
        self.into_realtime_with_epoch(queue_capacity, Instant::now())
    }

    /// Moves the provider onto its own task, ticking in real time at the telemetry rate into a bounded
    /// latest-wins queue: when the consumer falls behind, the oldest queued ticks are discarded (and
    /// counted), never buffered without bound. Tick content is identical to [`Self::next_tick`] except
    /// `capture_mono_ns` (tick and frame), which is the real emission time in ns since `clock_epoch` — pass
    /// the epoch of the application's shared monotonic clock so consumers can compute sample ages. Fails
    /// with [`SyntheticError::NoRuntime`] outside a tokio runtime.
    pub fn into_realtime_with_epoch(
        self,
        queue_capacity: usize,
        clock_epoch: Instant,
    ) -> Result<RealtimeStream, SyntheticError> {
        if queue_capacity == 0 {
            return Err(SyntheticError::config(
                "queue_capacity",
                "must be at least one tick",
            ));
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(SyntheticError::NoRuntime)?;
        let shared = Arc::new(RealtimeShared::new(queue_capacity));
        let task = runtime.spawn(run_realtime(self, Arc::clone(&shared), clock_epoch));
        Ok(RealtimeStream {
            shared,
            task: Some(task),
        })
    }

    fn mono_ns(&self, tick: u64) -> u64 {
        // A session of days at 1 kHz stays far below u64::MAX ns.
        MONO_START_NS.saturating_add((tick as f64 * self.dt_s * NANOS_PER_SECOND).round() as u64)
    }

    fn maybe_start_stall(&mut self, events: &mut Vec<SyntheticEvent>) {
        let next_et = self.session_ticks as f64 * self.dt_s;
        let due = self
            .scenario
            .stale
            .iter()
            .zip(self.stalls_done.iter_mut())
            .find(|(spec, done)| !**done && spec.start_et_s <= next_et);
        if let Some((spec, done)) = due {
            *done = true;
            // Stall lengths are validated finite and positive; at least one tick.
            let ticks = (spec.duration_s * self.scenario.telemetry_rate_hz)
                .round()
                .max(1.0) as u64;
            self.active_stall = Some(ActiveStall {
                mode: spec.mode,
                remaining_ticks: ticks,
            });
            tracing::debug!(component = "synthetic", mode = ?spec.mode, et_s = next_et, "source stall begins");
            events.push(SyntheticEvent::StallStarted { mode: spec.mode });
        }
    }

    fn stalled_tick(
        &mut self,
        tick: u64,
        mono: u64,
        mut events: Vec<SyntheticEvent>,
        mode: StaleMode,
    ) -> SyntheticTick {
        if let Some(stall) = self.active_stall.as_mut() {
            stall.remaining_ticks = stall.remaining_ticks.saturating_sub(1);
            if stall.remaining_ticks == 0 {
                self.active_stall = None;
                events.push(SyntheticEvent::StallEnded);
            }
        }
        let (frame, injection) = match (mode, &self.last_frame) {
            (StaleMode::FrozenFrames, Some((last, injection))) => {
                // Frozen shared memory: same content and source ET, new read (sequence, capture time).
                self.sequence = self.sequence.saturating_add(1);
                let mut frame = last.clone();
                frame.sequence = self.sequence;
                frame.capture_mono_ns = mono;
                (Some(frame), *injection)
            }
            _ => (None, None),
        };
        SyntheticTick {
            origin: self.origin(),
            tick,
            capture_mono_ns: mono,
            session_et_s: self.session_time_s(),
            frame,
            scoring: None,
            injection,
            stale: Some(mode),
            events,
        }
    }

    /// Advances the whole simulation by one step from `et_prev`.
    fn step(&mut self, et_prev: f64, events: &mut Vec<SyntheticEvent>) {
        let dt = self.dt_s;
        self.weather.step(
            &self.scenario.weather,
            et_prev + dt,
            dt,
            &mut self.weather_rng,
        );
        let wet_blend = self.weather.wet_blend();
        let grip = 1.0 - (1.0 - WET_GRIP) * wet_blend;
        self.plan_pit_stops();

        let outcomes: Vec<(bool, StepOutcome)> = {
            let ctx = SpeedContext {
                dry: &self.dry,
                wet: &self.wet,
                wet_blend,
                caution_cap_mps: self.flags.caution_cap_mps,
                grip,
            };
            self.cars
                .iter_mut()
                .map(|car| {
                    let was_stopped = matches!(car.pit, PitPhase::Stopped { .. });
                    let outcome = car.step(&self.track, &self.vehicle, &ctx, et_prev, dt);
                    (was_stopped, outcome)
                })
                .collect()
        };
        for (index, (was_stopped, outcome)) in outcomes.iter().enumerate() {
            self.after_step(index, *was_stopped, outcome, events);
        }

        let player_step = outcomes
            .get(self.player)
            .map_or(0.0, |(_, outcome)| outcome.distance_m);
        let input = self.tick_input(player_step, grip);
        self.dynamics
            .update(&self.vehicle, &input, &mut self.sim_rng);
        self.check_excursion(events);
        if let Some(car) = self.cars.get(self.player)
            && let Some(start) = car.timing.lap_start_et_s
        {
            let elapsed = et_prev + dt - start;
            self.lap_trace.record(car.s_m, elapsed, player_step, dt);
        }
    }

    fn tick_input(&self, distance_m: f64, grip: f64) -> TickInput {
        let car = &self.cars[self.player];
        let pit = self.track.pit();
        TickInput {
            dt_s: self.dt_s,
            v_mps: car.v_mps,
            a_long_mps2: car.a_long_mps2,
            distance_m,
            point: self.track.point(car.s_m),
            grip,
            wetness: self.weather.wetness,
            air_temp_c: self.weather.air_temp_c,
            track_temp_c: self.weather.track_temp_c,
            stopped: matches!(car.pit, PitPhase::Stopped { .. }),
            in_pit_lane: car.pit.in_pit_lane(),
            pit_speed_limit_mps: pit.speed_limit_mps,
            half_width_m: self.track.half_width_m(),
        }
    }

    /// Flags the next scheduled stop of every car as due.
    fn plan_pit_stops(&mut self) {
        let player = self.player;
        if let Some(car) = self.cars.get_mut(player)
            && car.pit == PitPhase::None
            && car.started()
        {
            while let Some(stop) = self.scenario.pit_stops.get(self.next_pit_stop) {
                let lap = car.current_lap();
                if lap < stop.lap {
                    break;
                }
                self.next_pit_stop += 1;
                if lap > stop.lap {
                    tracing::warn!(
                        component = "synthetic",
                        lap = stop.lap,
                        "scheduled pit stop missed"
                    );
                    continue;
                }
                car.pit = PitPhase::Planned;
                self.pending_service = Some(ServiceRequest {
                    refuel_l: stop.refuel_l,
                    change_tyres: stop.change_tyres,
                    compound: stop.compound,
                });
                break;
            }
        }
        for (index, (car, plan)) in self.cars.iter_mut().zip(&self.strategies).enumerate() {
            if index == player || car.pit != PitPhase::None || !car.started() {
                continue;
            }
            if car.current_lap() >= plan.next_stop_lap {
                car.pit = PitPhase::Planned;
                car.next_service_s = self
                    .field_rng
                    .random_range(COMPETITOR_SERVICE_S.0..=COMPETITOR_SERVICE_S.1);
            }
        }
    }

    fn after_step(
        &mut self,
        index: usize,
        was_stopped: bool,
        outcome: &StepOutcome,
        events: &mut Vec<SyntheticEvent>,
    ) {
        let is_player = index == self.player;
        let length = self.track.length_m();
        let sigma = self.scenario.driver.lap_pace_sigma;
        let Some(car) = self.cars.get_mut(index) else {
            return;
        };
        let slot_id = car.slot_id;
        if outcome.line_crossed_at.is_some() {
            let base = self.strategies.get(index).map_or(1.0, |s| s.base_pace);
            car.pace = clamp_pace(base * (1.0 + sigma * gaussian(&mut self.sim_rng)));
            if is_player {
                self.dynamics
                    .new_lap(self.scenario.driver.fuel_sigma, &mut self.sim_rng);
                match &outcome.completed_lap {
                    Some(lap) => {
                        let new_best = car.timing.best_lap_s == Some(lap.lap_time_s);
                        self.lap_trace.complete(lap, new_best);
                    }
                    None => self.lap_trace.start_lap(),
                }
            }
        }
        if let Some(lap) = outcome.completed_lap {
            events.push(SyntheticEvent::LapCompleted {
                slot_id,
                is_player,
                lap,
            });
        }
        if outcome.entered_pit_lane {
            events.push(SyntheticEvent::PitLaneEntered { slot_id });
        }
        if outcome.stopped_in_box {
            let service_s = if is_player {
                let request = self.pending_service.take().unwrap_or(ServiceRequest {
                    refuel_l: Some(0.0),
                    change_tyres: false,
                    compound: None,
                });
                let duration = self.dynamics.begin_service(&self.vehicle, request);
                car.pit = PitPhase::Stopped {
                    remaining_s: duration,
                };
                duration
            } else {
                if let Some(plan) = self.strategies.get_mut(index) {
                    plan.stint_start_progress_m = car.progress_m(length);
                    plan.next_stop_lap = car.current_lap().saturating_add(plan.stint_laps);
                }
                car.next_service_s
            };
            events.push(SyntheticEvent::PitBoxStop { slot_id, service_s });
        }
        if was_stopped && !matches!(car.pit, PitPhase::Stopped { .. }) && is_player {
            self.dynamics.end_service();
        }
        if outcome.exited_pit_lane {
            events.push(SyntheticEvent::PitLaneExited { slot_id });
        }
    }

    /// Starts the scheduled track-limits excursion of the current lap.
    fn check_excursion(&mut self, events: &mut Vec<SyntheticEvent>) {
        let Some(car) = self.cars.get_mut(self.player) else {
            return;
        };
        let lap = car.current_lap();
        let due = lap > 0
            && self.scenario.invalid_laps.contains(&lap)
            && !self.excursions_done.contains(&lap)
            && !car.pit.in_pit_lane()
            && car.s_m >= EXCURSION_LAP_FRACTION * self.track.length_m();
        if due {
            self.excursions_done.push(lap);
            self.dynamics.start_excursion();
            car.timing.lap_valid = false;
            events.push(SyntheticEvent::TrackLimits { lap });
        }
    }

    /// Lateral offset of car `index` from the racing line (pit lane and excursions), + = right (m).
    fn lateral_of(&self, index: usize) -> f64 {
        let Some(car) = self.cars.get(index) else {
            return 0.0;
        };
        let pit = if car.pit.in_pit_lane() {
            self.track.pit().lateral_at(car.s_m)
        } else {
            0.0
        };
        let excursion = if index == self.player {
            self.dynamics.lateral_m
        } else {
            0.0
        };
        pit + excursion
    }

    fn build_scoring(&self, et_s: f64) -> ScoringSnapshot {
        let length = self.track.length_m();
        let per_lap = energy_per_lap_frac(&self.vehicle);
        let positions: Vec<(f64, f64)> = self
            .cars
            .iter()
            .enumerate()
            .map(|(i, car)| {
                let (x, _, z) = self.track.world_position(car.s_m, self.lateral_of(i));
                (x, z)
            })
            .collect();
        let energy: Vec<Option<f64>> = self
            .cars
            .iter()
            .zip(&self.strategies)
            .enumerate()
            .map(|(i, (car, plan))| {
                if i == self.player {
                    Some(if self.vehicle.virtual_energy {
                        self.dynamics.virtual_energy
                    } else {
                        self.dynamics.fuel_l / self.vehicle.fuel_capacity_l
                    })
                } else {
                    let laps = (car.progress_m(length) - plan.stint_start_progress_m) / length;
                    Some((1.0 - laps.max(0.0) * per_lap).clamp(0.0, 1.0))
                }
            })
            .collect();
        session::build_scoring(&ScoringInputs {
            et_s,
            track: &self.track,
            session: &self.scenario.session,
            flags: &self.flags,
            weather: &self.weather,
            cars: &self.cars,
            identities: &self.identities,
            energy_fracs: &energy,
            positions: &positions,
            reference_lap_s: self.dry.lap_time_s(),
        })
    }

    fn frame_context(&self, et_s: f64) -> FrameContext<'_> {
        let car = &self.cars[self.player];
        let lateral_m = self.lateral_of(self.player);
        let delta_best_s = car
            .timing
            .lap_start_et_s
            .and_then(|start| self.lap_trace.delta(car.s_m, et_s - start));
        FrameContext {
            et_s,
            dt_s: self.dt_s,
            track: &self.track,
            vehicle: &self.vehicle,
            car,
            dynamics: &self.dynamics,
            scoring: &self.scoring,
            weather: &self.weather,
            position: self.track.world_position(car.s_m, lateral_m),
            heading_rad: self.track.point(car.s_m).heading_rad,
            lateral_m,
            delta_best_s,
            vehicle_count: self.cars.len(),
            channel_fill: self.scenario.channel_fill,
        }
    }
}

/// Energy (virtual energy, else fuel) used per nominal lap, as a fraction of a full tank.
fn energy_per_lap_frac(vehicle: &VehicleProfile) -> f64 {
    let frac = if vehicle.virtual_energy {
        vehicle.ve_per_lap_frac
    } else {
        vehicle.fuel_per_lap_l / vehicle.fuel_capacity_l
    };
    frac.max(f64::EPSILON)
}

/// Scoring identity of the car on grid slot `index`; every string carries the synthetic prefix.
fn identity(vehicle: &VehicleProfile, index: usize, is_player: bool) -> CarIdentity {
    let number = index + 1;
    CarIdentity {
        driver: if is_player {
            synthetic_name("Player")
        } else {
            synthetic_name(&format!("Driver {number:02}"))
        },
        vehicle_name: if is_player {
            vehicle.name.clone()
        } else {
            synthetic_name(&format!("{} #{}", vehicle.model, 100 + number))
        },
        vehicle_filename: vehicle.filename.clone(),
        class_name: vehicle.class_name.clone(),
        pit_group: synthetic_name(&format!("Team {number:02}")),
        // Grid place, ≤ MAX_COMPETITORS + 1.
        qualification: i32::try_from(number).unwrap_or(i32::MAX),
    }
}

/// Bounded latest-wins tick queue shared by the real-time task (producer) and [`RealtimeStream`].
#[derive(Debug)]
struct RealtimeShared {
    queue: Mutex<VecDeque<SyntheticTick>>,
    capacity: usize,
    notify: Notify,
    closed: AtomicBool,
    dropped: AtomicU64,
}

impl RealtimeShared {
    fn new(capacity: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
            notify: Notify::new(),
            closed: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<SyntheticTick>> {
        // Push and pop cannot leave the deque half-updated, so a poisoned lock still holds a valid queue.
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `tick`; on a full queue the OLDEST tick is discarded (latest wins) and counted. Its
    /// non-repeatable content is carried into the next tick the consumer will see (see
    /// [`carry_evicted`]), so a slow consumer loses frames but no scoring update or discrete event.
    fn push(&self, mut tick: SyntheticTick) {
        let (evicted, lost_events) = {
            let mut queue = self.lock();
            let evicted = if queue.len() >= self.capacity {
                queue.pop_front()
            } else {
                None
            };
            let was_evicted = evicted.is_some();
            let lost_events = match evicted {
                Some(old) => match queue.front_mut() {
                    Some(front) => carry_evicted(old, front),
                    None => carry_evicted(old, &mut tick),
                },
                None => 0,
            };
            queue.push_back(tick);
            (was_evicted, lost_events)
        };
        if evicted {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        if lost_events > 0 {
            tracing::warn!(
                component = "synthetic.realtime",
                lost_events,
                cap = MAX_CARRIED_EVENTS,
                "consumer too slow: oldest carried events discarded"
            );
        }
        self.notify.notify_one();
    }

    fn pop(&self) -> Option<SyntheticTick> {
        self.lock().pop_front()
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_one();
    }
}

/// Upper bound on the events one queued tick carries for evicted ticks (bounded memory when the consumer
/// stalls); far above the events of a real backlog (a few per lap).
const MAX_CARRIED_EVENTS: usize = 1024;

/// Moves the non-repeatable content of an evicted tick into `next` (the next tick the consumer sees):
/// its scoring snapshot when `next` has none (a newer snapshot supersedes it, snapshots are full
/// states), and its events, prepended in order. Returns how many events had to be discarded to stay
/// within [`MAX_CARRIED_EVENTS`] (oldest first).
fn carry_evicted(evicted: SyntheticTick, next: &mut SyntheticTick) -> usize {
    if next.scoring.is_none() {
        next.scoring = evicted.scoring;
    }
    if evicted.events.is_empty() {
        return 0;
    }
    let mut events = evicted.events;
    events.append(&mut next.events);
    let excess = events.len().saturating_sub(MAX_CARRIED_EVENTS);
    if excess > 0 {
        events.drain(..excess);
    }
    next.events = events;
    excess
}

/// Marks the queue closed when the producer task ends for any reason (return, error, panic or abort).
struct CloseOnDrop(Arc<RealtimeShared>);

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// A provider ticking in real time on its own tokio task. Dropping the stream stops the task.
#[derive(Debug)]
pub struct RealtimeStream {
    shared: Arc<RealtimeShared>,
    task: Option<JoinHandle<Result<(), SyntheticError>>>,
}

impl RealtimeStream {
    /// The oldest queued tick; `None` once the provider task has ended and the queue is drained (see
    /// [`Self::finish`] for why it ended).
    pub async fn recv(&mut self) -> Option<SyntheticTick> {
        loop {
            if let Some(tick) = self.shared.pop() {
                return Some(tick);
            }
            if self.shared.closed.load(Ordering::Acquire) {
                // A tick pushed just before the close is still delivered.
                return self.shared.pop();
            }
            // A notification sent between the checks above and this await is stored as a permit, so no
            // wake-up is lost.
            self.shared.notify.notified().await;
        }
    }

    /// Ticks discarded because the consumer did not keep up (the oldest queued ticks are discarded).
    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }

    /// Stops the provider task.
    pub fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    /// Stops the provider task (if it is still running) and reports how it ended: `Ok(())` for a normal
    /// stop, the generator's error (root cause preserved) if it failed, or
    /// [`SyntheticError::RealtimeTask`] if it panicked.
    pub async fn finish(mut self) -> Result<(), SyntheticError> {
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        // Aborting a task that already returned keeps its result.
        task.abort();
        match task.await {
            Ok(result) => result,
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(SyntheticError::RealtimeTask(error)),
        }
    }
}

impl Drop for RealtimeStream {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Most ticks the real-time task produces in one wake-up to catch up with its schedule; beyond that the
/// schedule is moved forward instead (the task was starved, e.g. by a blocked runtime).
const MAX_CATCH_UP: Duration = Duration::from_millis(250);

/// Nanoseconds in `d`, saturating at `u64::MAX`.
fn duration_ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// Produces tick `n` at `start + n·dt` against an absolute schedule, so late timer wake-ups (Windows timer
/// granularity is ~15.6 ms) are caught up instead of slowing the stream down. Every tick is stamped with
/// the real time of its emission, in ns since `clock_epoch`.
async fn run_realtime(
    mut provider: SyntheticTelemetryProvider,
    shared: Arc<RealtimeShared>,
    clock_epoch: Instant,
) -> Result<(), SyntheticError> {
    let _close = CloseOnDrop(Arc::clone(&shared));
    let dt_s = provider.dt_s;
    // Small positive tick count (≥ 1).
    let max_catch_up = (MAX_CATCH_UP.as_secs_f64() / dt_s).ceil().max(1.0) as u64;
    let mut schedule_start = tokio::time::Instant::now();
    let mut produced: u64 = 0;
    let mut last_capture_ns: u64 = 0;
    loop {
        let deadline = schedule_start + Duration::from_secs_f64(produced as f64 * dt_s);
        tokio::time::sleep_until(deadline).await;
        // Tick n is due at n·dt: ticks 0..=floor(elapsed / dt) are due now.
        let elapsed_s = schedule_start.elapsed().as_secs_f64();
        let mut due = (elapsed_s / dt_s + EPOCH_EPSILON).floor() as u64 + 1;
        let behind = due.saturating_sub(produced);
        if behind > max_catch_up {
            let skipped = behind - max_catch_up;
            schedule_start += Duration::from_secs_f64(skipped as f64 * dt_s);
            due = produced + max_catch_up;
            tracing::warn!(
                component = "synthetic",
                skipped_ticks = skipped,
                "real-time synthetic provider fell behind; schedule moved forward"
            );
        }
        while produced < due {
            let mut tick = provider.next_tick().inspect_err(|error| {
                tracing::error!(component = "synthetic", %error, "synthetic provider failed");
            })?;
            produced += 1;
            // Strictly increasing, never zero.
            let capture_ns =
                duration_ns(clock_epoch.elapsed()).max(last_capture_ns.saturating_add(1));
            last_capture_ns = capture_ns;
            tick.capture_mono_ns = capture_ns;
            if let Some(frame) = tick.frame.as_mut() {
                frame.capture_mono_ns = capture_ns;
            }
            shared.push(tick);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::TestMode;
    use telemetry_model::frame::FrameError;
    use telemetry_model::value::MAX_STR_VALUE_BYTES;

    /// Drives one lap of `length_m` at constant `v_mps` with time step `dt_s` through the trace and
    /// returns the lap time.
    fn drive_lap(trace: &mut LapTrace, length_m: f64, v_mps: f64, dt_s: f64) -> f64 {
        let step_m = v_mps * dt_s;
        let lap_time_s = length_m / v_mps;
        let mut s_m = 0.0;
        let mut elapsed_s = 0.0;
        while s_m + step_m < length_m {
            s_m += step_m;
            elapsed_s += dt_s;
            trace.record(s_m, elapsed_s, step_m, dt_s);
        }
        lap_time_s
    }

    fn lap(n: u32, lap_time_s: f64) -> CompletedLap {
        CompletedLap {
            lap: n,
            lap_time_s,
            sectors_s: None,
            valid: true,
        }
    }

    /// Regression (review pass 2, F2): when one step crossed the last buckets and the line, the new best
    /// lap was never stored and an older, slower lap stayed the reference.
    #[test]
    fn lap_trace_stores_the_new_best_lap_even_with_large_steps() {
        const LENGTH_M: f64 = 1_003.0;
        let mut trace = LapTrace::new(LENGTH_M);
        trace.start_lap();
        // 2 Hz at 50 / 60 m/s: 25-30 m steps skip several buckets, including the last ones.
        let slow = drive_lap(&mut trace, LENGTH_M, 50.0, 0.5);
        trace.complete(&lap(1, slow), true);
        assert!(trace.best.is_some());
        let fast = drive_lap(&mut trace, LENGTH_M, 60.0, 0.5);
        trace.complete(&lap(2, fast), true);
        // Measured against the fast lap: zero delta along an identical lap, including the short last segment.
        for s_m in [5.0, 500.0, 995.0, 1_001.0, 1_002.9] {
            let delta = trace.delta(s_m, s_m / 60.0).expect("delta");
            assert!(delta.abs() < 1e-9, "s {s_m}: {delta}");
        }
    }

    #[test]
    fn lap_trace_clears_the_reference_when_the_new_best_trace_is_incomplete() {
        let mut trace = LapTrace::new(100.0);
        trace.start_lap();
        let t = drive_lap(&mut trace, 100.0, 10.0, 0.1);
        trace.complete(&lap(1, t), true);
        assert!(trace.best.is_some());
        // A lap that never started recording (no start_lap): incomplete.
        trace.last_index = None;
        trace.current.iter_mut().for_each(|b| *b = None);
        trace.complete(&lap(2, t - 1.0), true);
        assert!(trace.best.is_none());
    }

    /// Regression (review pass 2, F12): a generator failure in real-time mode used to end the stream
    /// silently; `finish` must return the root cause.
    #[tokio::test]
    async fn realtime_generator_failure_is_returned_by_finish() {
        let mut provider =
            SyntheticTelemetryProvider::new(TestMode::acknowledge(), Scenario::normal_lap(1))
                .expect("provider");
        // Corrupt the provider so that the first frame cannot be written.
        provider.vehicle.name = "x".repeat(MAX_STR_VALUE_BYTES + 1);
        let mut stream = provider.into_realtime(4).expect("stream");
        assert!(stream.recv().await.is_none());
        let result = stream.finish().await;
        assert!(
            matches!(
                result,
                Err(SyntheticError::Frame(FrameError::StringTooLong { .. }))
            ),
            "{result:?}"
        );
    }

    /// Regression (review pass 2, F14): absent scoring fields were published as fabricated Valid defaults
    /// (`unwrap_or(true/false/0/LapAndTime)`, out-of-range sector → S3); they are Unavailable now.
    #[test]
    fn absent_scoring_fields_are_unavailable_not_defaulted() {
        use telemetry_model::ch;
        use telemetry_model::value::Quality;
        let scenario = Scenario::builder(5).build().expect("scenario");
        let mut p =
            SyntheticTelemetryProvider::new(TestMode::acknowledge(), scenario).expect("provider");
        let session = &mut p.scoring.state.session;
        session.in_realtime = None;
        session.start_light = None;
        session.fixed_setup = None;
        let player = p.scoring.player_index;
        let row = p
            .scoring
            .state
            .vehicles
            .get_mut(player)
            .expect("player row");
        row.in_pits = None;
        row.in_garage_stall = None;
        row.count_lap_flag = None;
        row.sector = Some(0);
        let mut frame = TelemetryFrame::new(1, 1, Some(0.0));
        writer::write_frame(&mut frame, &p.frame_context(0.0)).expect("write");
        for id in [
            ch::IN_REALTIME,
            ch::START_LIGHT,
            ch::FIXED_SETUP,
            ch::IN_PITS,
            ch::IN_GARAGE,
            ch::COUNT_LAP_FLAG,
            ch::SECTOR_SCORING_RAW,
            ch::SECTOR_SCORING,
        ] {
            assert_eq!(
                frame.get(id).quality,
                Quality::Unavailable,
                "0x{:04X}",
                id.0
            );
        }
    }

    /// Regression (review pass 3): a full latest-wins queue drops frames but carries the evicted
    /// ticks' scoring snapshot and events into the next tick the consumer sees.
    #[test]
    fn evicted_ticks_keep_their_scoring_and_events() {
        const POP_EVERY: usize = 37;
        let mut p = SyntheticTelemetryProvider::new(TestMode::acknowledge(), Scenario::pit_lap(3))
            .expect("provider");
        let ticks = (1.6 * p.reference_lap_time_s() * p.scenario().telemetry_rate_hz) as usize;
        let shared = RealtimeShared::new(1);
        let (mut pushed_events, mut received_events, mut laps) = (0_usize, 0_usize, 0_usize);
        let mut pending_scoring: Option<f64> = None;
        let check_pop = |shared: &RealtimeShared, pending: &mut Option<f64>| {
            let t = shared.pop().expect("queued tick");
            assert_eq!(t.scoring.as_ref().map(|s| s.et_s), pending.take());
            t.events.len()
        };
        for i in 0..ticks {
            let t = p.next_tick().expect("tick");
            pushed_events += t.events.len();
            laps += t
                .events
                .iter()
                .filter(|e| matches!(e, SyntheticEvent::LapCompleted { .. }))
                .count();
            if let Some(s) = &t.scoring {
                pending_scoring = Some(s.et_s);
            }
            shared.push(t);
            if i % POP_EVERY == POP_EVERY - 1 {
                received_events += check_pop(&shared, &mut pending_scoring);
            }
        }
        if shared.lock().front().is_some() {
            received_events += check_pop(&shared, &mut pending_scoring);
        }
        assert!(laps > 0, "the run completes laps");
        assert!(shared.dropped.load(Ordering::Relaxed) > 0);
        assert_eq!(received_events, pushed_events);
    }

    #[test]
    fn carried_events_are_bounded() {
        let mut p =
            SyntheticTelemetryProvider::new(TestMode::acknowledge(), Scenario::normal_lap(1))
                .expect("provider");
        let mut old = p.next_tick().expect("tick");
        let mut next = p.next_tick().expect("tick");
        let marker = SyntheticEvent::StallEnded;
        old.events = vec![marker.clone(); MAX_CARRIED_EVENTS];
        next.events = vec![marker; 3];
        next.scoring = None;
        let had_scoring = old.scoring.is_some();
        assert_eq!(carry_evicted(old, &mut next), 3);
        assert_eq!(next.events.len(), MAX_CARRIED_EVENTS);
        assert_eq!(next.scoring.is_some(), had_scoring);
    }
}

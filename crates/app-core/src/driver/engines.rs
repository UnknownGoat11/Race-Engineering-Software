//! `EngineHub`: the pure, deterministic per-frame engine sequence.
//!
//! Executes the full race-engines suite in the exact sequence specified by BLUEPRINT.md §6:
//! 1. `timing.update(TimingSample::from_frame)` + lap conditions capture on completed laps.
//! 2. `race.update(&scoring, &RaceSample::from_frame)` + pit/flag session events.
//! 3. Lap completion hooks: `fuel.lap_completed`, `energy.lap_completed`, `stint.lap_completed`.
//! 4. Per-frame engine updates: `fuel.update`, `stint.update`, `tyre.update`, `brake.update`, `energy.update`.
//! 5. Snapshots: `timing`, `fuel` (via `estimate(&FuelContext)`), `stint` (+ `stint_max_laps`), `race`, `energy`.
//! 6. Channel publication: `publish::publish` and `energy::publish_energy` into the telemetry frame.
//! 7. Alerts evaluation: `AlertEngine::evaluate_frame` yielding [`AlertNotice`]s and [`SessionEvent`]s.
//!
//! Unavailable inputs never yield valid zeros.

use serde::Serialize;
use telemetry_model::ch;
use telemetry_model::events::SessionEvent;
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::scoring::ScoringState;
use telemetry_model::value::Value;
use thiserror::Error;
use tracing::{debug, warn};

use race_engines::aids::{AidsEngine, AidsSample, AidsSnapshot};
use race_engines::alert::{
    AlertClock, AlertConfig, AlertConfigError, AlertEngine, AlertNotice, AlertState,
};
use race_engines::brake::{BrakeConfig, BrakeConfigError, BrakeEngine, BrakeSample, BrakeSnapshot};
use race_engines::common::{CompatibilityKey, Metric, finite, frame_f64};
use race_engines::energy::{
    EnergyConfig, EnergyConfigError, EnergyEngine, EnergySample, EnergySnapshot, publish_energy,
};
use race_engines::fuel::{
    FuelConfig, FuelContext, FuelEngine, FuelEstimate, FuelLapInfo, FuelSample, RaceFinish,
};
use race_engines::publish::{EngineOutputs, ExtendedOutputs, publish, publish_extended};
use race_engines::race::{LapConditions, PitPhase, RaceEngine, RaceSample, RaceSnapshot};
use race_engines::stint::{
    StintEngine, StintEvent, StintLap, StintSample, StintSnapshot, stint_max_laps,
};
use race_engines::timing::{
    LapId, ReferenceKind, ResetReason, TimingConfig, TimingEngine, TimingError, TimingSample,
    TimingSnapshot,
};
use race_engines::tyre::{TyreConfig, TyreConfigError, TyreEngine, TyreSample, TyreSnapshot};

/// Tracing component name for logging.
const COMPONENT: &str = "app_core::driver::engines";

/// Errors that can occur during [`EngineHub`] initialization or configuration.
#[derive(Debug, Error)]
pub enum EngineHubError {
    /// Timing engine creation or configuration error.
    #[error("timing engine initialization failed: {0}")]
    Timing(#[from] TimingError),
    /// Alert engine creation or configuration error.
    #[error("alert engine initialization failed: {0}")]
    Alert(#[from] AlertConfigError),
    /// Brake engine creation or configuration error.
    #[error("brake engine initialization failed: {0}")]
    Brake(#[from] BrakeConfigError),
    /// Energy engine creation or configuration error.
    #[error("energy engine initialization failed: {0}")]
    Energy(#[from] EnergyConfigError),
    /// Tyre engine creation or configuration error.
    #[error("tyre engine initialization failed: {0}")]
    Tyre(#[from] TyreConfigError),
}

/// Configuration options for initializing the [`EngineHub`].
#[derive(Debug, Clone, Default)]
pub struct EngineHubConfig {
    /// Timing engine configuration.
    pub timing: TimingConfig,
    /// Fuel engine configuration.
    pub fuel: FuelConfig,
    /// Tyre engine configuration.
    pub tyre: TyreConfig,
    /// Brake engine configuration.
    pub brake: BrakeConfig,
    /// Energy engine configuration.
    pub energy: EnergyConfig,
    /// Alert engine configuration.
    pub alert: AlertConfig,
}

/// Outcome of processing one frame through the [`EngineHub`].
#[derive(Debug, Clone)]
pub struct EngineFrameOutcome {
    /// Session events generated during this frame (laps, sectors, pit stops, flags, alerts).
    pub events: Vec<SessionEvent>,
    /// Alert state changes (raised or cleared) from this frame.
    pub alert_notices: Vec<AlertNotice>,
    /// Timing engine snapshot.
    pub timing: TimingSnapshot,
    /// Fuel engine estimate.
    pub fuel: FuelEstimate,
    /// Stint engine snapshot.
    pub stint: StintSnapshot,
    /// Race engine snapshot (if scoring / race state is available).
    pub race: Option<RaceSnapshot>,
    /// Energy engine snapshot.
    pub energy: EnergySnapshot,
    /// Tyre engine snapshot.
    pub tyre: TyreSnapshot,
    /// Brake engine snapshot.
    pub brake: BrakeSnapshot,
    /// Driver-aid duty cycles snapshot.
    pub aids: AidsSnapshot,
    /// Stint boundary events generated during this frame.
    pub stint_events: Vec<StintEvent>,
    /// Stint maximum laps calculated from fuel and energy limits.
    pub stint_max_laps: Metric<f64>,
    /// Total publication errors encountered during this frame.
    pub publish_errors: usize,
}

impl EngineFrameOutcome {
    /// Converts the current snapshot state into a serializable DTO for `core:engine_state` JSON events.
    pub fn to_engine_state_dto(&self) -> EngineStateDto {
        EngineStateDto {
            timing: self.timing.clone(),
            fuel: self.fuel.clone(),
            stint: self.stint.clone(),
            race: self.race.clone(),
            energy: self.energy,
            tyre: self.tyre,
            brake: self.brake,
            aids: self.aids,
        }
    }
}

/// Serializable snapshot of all engine states for UI inspection and debugging.
#[derive(Debug, Clone, Serialize)]
pub struct EngineStateDto {
    /// Timing snapshot.
    pub timing: TimingSnapshot,
    /// Fuel estimate snapshot.
    pub fuel: FuelEstimate,
    /// Stint snapshot.
    pub stint: StintSnapshot,
    /// Race snapshot.
    pub race: Option<RaceSnapshot>,
    /// Energy snapshot.
    pub energy: EnergySnapshot,
    /// Tyre snapshot.
    pub tyre: TyreSnapshot,
    /// Brake snapshot.
    pub brake: BrakeSnapshot,
    /// Driver-aid duty cycles snapshot.
    pub aids: AidsSnapshot,
}

/// Pure, deterministic hub coordinating all calculation engines.
pub struct EngineHub {
    pub timing: TimingEngine,
    pub race: RaceEngine,
    pub fuel: FuelEngine,
    pub stint: StintEngine,
    pub tyre: TyreEngine,
    pub brake: BrakeEngine,
    pub energy: EnergyEngine,
    pub aids: AidsEngine,
    pub alert: AlertEngine,
    publish_errors_count: u64,
    last_pb_lap: Option<LapId>,
}

impl Default for EngineHub {
    fn default() -> Self {
        match Self::new() {
            Ok(hub) => hub,
            Err(err) => {
                panic!("default EngineHub configuration must be valid: {err}");
            }
        }
    }
}

impl EngineHub {
    /// Creates a new engine hub with default engine configurations.
    pub fn new() -> Result<Self, EngineHubError> {
        Self::with_config(EngineHubConfig::default())
    }

    /// Creates a new engine hub with explicit engine configurations.
    pub fn with_config(config: EngineHubConfig) -> Result<Self, EngineHubError> {
        let timing = TimingEngine::new(config.timing)?;
        let race = RaceEngine::new();
        let fuel = FuelEngine::new(config.fuel);
        let stint = StintEngine::new();
        let tyre = TyreEngine::new(config.tyre)?;
        let brake = BrakeEngine::new(config.brake)?;
        let energy = EnergyEngine::new(config.energy)?;
        let aids = AidsEngine::new();
        let alert = AlertEngine::new(&config.alert)?;

        Ok(Self {
            timing,
            race,
            fuel,
            stint,
            tyre,
            brake,
            energy,
            aids,
            alert,
            publish_errors_count: 0,
            last_pb_lap: None,
        })
    }

    /// Cumulative count of publish errors encountered across frames.
    pub fn publish_errors_count(&self) -> u64 {
        self.publish_errors_count
    }

    /// Sets the compatibility context (track, vehicle, class, session).
    ///
    /// Signals a reset to the timing engine when the context changes.
    pub fn set_context(&mut self, key: CompatibilityKey) -> Option<ResetReason> {
        self.last_pb_lap = None;
        self.aids = AidsEngine::new();
        self.timing.set_context(key)
    }

    /// Selects the reference lap kind for delta computation.
    pub fn select_reference(&mut self, reference: ReferenceKind) -> Result<(), TimingError> {
        self.timing.select_reference(reference)
    }

    /// Sets the session best lap time from external cars (e.g. from scoring).
    pub fn set_external_session_best(&mut self, lap_s: Option<f64>) {
        self.timing.set_external_session_best(lap_s);
    }

    /// Evaluates alert rules when the pipeline is idle or between frames (housekeeping 10 Hz tick).
    pub fn evaluate_idle_alerts(
        &mut self,
        clock: AlertClock,
        last_frame: &TelemetryFrame,
    ) -> Vec<AlertNotice> {
        let lap = self.timing.snapshot().lap_number;
        self.alert.evaluate_frame(clock, lap, last_frame)
    }

    /// Processes one telemetry frame through the deterministic engine pipeline.
    ///
    /// Updates all engines in order, writes calculated metrics into `frame`, and
    /// produces the [`EngineFrameOutcome`].
    pub fn process_frame(
        &mut self,
        frame: &mut TelemetryFrame,
        scoring: Option<&ScoringState>,
    ) -> EngineFrameOutcome {
        let mut events: Vec<SessionEvent> = Vec::new();

        // ---------------------------------------------------------------------
        // Step 1: Timing Engine update
        // ---------------------------------------------------------------------
        let timing_sample = TimingSample::from_frame(frame);
        let timing_update = if let Some(ref s) = timing_sample {
            self.timing.update(s)
        } else {
            Default::default()
        };

        // If a lap completed, take conditions from the race engine:
        let completed_lap_conditions = if timing_update.completed_lap.is_some() {
            Some(self.race.take_lap_conditions())
        } else {
            None
        };

        // ---------------------------------------------------------------------
        // Step 2: Race Engine update
        // ---------------------------------------------------------------------
        let default_scoring;
        let scoring_ref = match scoring {
            Some(s) => s,
            None => {
                default_scoring = ScoringState::default();
                &default_scoring
            }
        };

        if let Some(race_sample) = RaceSample::from_frame(frame) {
            let race_update = self.race.update(scoring_ref, &race_sample);

            for pit_ev in race_update.pit_events {
                if let Some(sev) = pit_ev.to_session_event() {
                    events.push(sev);
                }
            }

            if let Some((from, to)) = race_update.flag_change {
                events.push(SessionEvent::RaceFlagChanged { from, to });
            }
        }

        // ---------------------------------------------------------------------
        // Step 3: Per-frame Telemetry Updates for Fuel, Energy, Aids
        // (Per fuel.rs / aids.rs: call lap_completed right after the sample on which it completed)
        // ---------------------------------------------------------------------
        if let Some(fuel_sample) = FuelSample::from_frame(frame) {
            self.fuel.update(&fuel_sample);
        }

        let lap_progress = frame_f64(frame, ch::LAP_PROGRESS);
        if let Some(energy_sample) = EnergySample::from_frame(frame, lap_progress) {
            self.energy.update(&energy_sample);
        }

        if let Some(aids_sample) = AidsSample::from_frame(frame) {
            self.aids.update(&aids_sample);
        }

        // ---------------------------------------------------------------------
        // Step 4: Lap-completion hooks (Fuel, Energy, Stint, Aids)
        // ---------------------------------------------------------------------
        if let Some(ref lap_rec) = timing_update.completed_lap {
            self.aids.lap_completed();

            let conditions = completed_lap_conditions.unwrap_or(LapConditions {
                formation: false,
                caution: false,
            });

            let fuel_lap_info =
                FuelLapInfo::from_record(lap_rec, conditions.formation, conditions.caution);
            let fuel_lap = self.fuel.lap_completed(fuel_lap_info);

            let energy_included = fuel_lap.exclusion.is_none();
            let energy_lap = self.energy.lap_completed(energy_included);

            let stint_lap =
                StintLap::from_laps(lap_rec, Some(&fuel_lap), conditions, energy_lap.ve_used);
            self.stint.lap_completed(&stint_lap);

            let lap_num = match lap_rec.lap_number {
                Some(n) if n >= 0 => n as u32,
                _ => 0,
            };

            events.push(SessionEvent::LapCompleted {
                lap: lap_num,
                time_s: lap_rec.time_s,
                valid: !lap_rec.flags.invalidated,
            });

            // Check if this lap became the new personal best:
            let timing_snap = self.timing.snapshot();
            if timing_snap.personal_best_lap == Some(lap_rec.id)
                && self.last_pb_lap != Some(lap_rec.id)
            {
                self.last_pb_lap = Some(lap_rec.id);
                if let Some(t) = lap_rec.time_s {
                    events.push(SessionEvent::PersonalBest {
                        lap: lap_num,
                        time_s: t,
                    });
                }
            }
        }

        // Sector completions:
        for sector in &timing_update.completed_sectors {
            events.push(SessionEvent::SectorCompleted {
                lap: sector.lap_id.0,
                sector: (sector.index as u8).saturating_add(1),
                time_s: Some(sector.time_s),
            });
        }

        // ---------------------------------------------------------------------
        // Step 5: Per-frame engine updates (Stint, Tyre, Brake)
        // ---------------------------------------------------------------------
        let pit_phase = self
            .race
            .snapshot()
            .map(|s| s.pit.phase)
            .unwrap_or(PitPhase::OnTrack);

        let et_s = finite(frame.source_et_s)
            .or_else(|| frame_f64(frame, ch::ELAPSED_TIME))
            .unwrap_or(0.0);

        let tyre_wear = [
            frame_f64(frame, ch::TYRE_WEAR_FL),
            frame_f64(frame, ch::TYRE_WEAR_FR),
            frame_f64(frame, ch::TYRE_WEAR_RL),
            frame_f64(frame, ch::TYRE_WEAR_RR),
        ];

        let front_compound = match frame.get(ch::TYRE_COMPOUND_FRONT).value {
            Value::Str(ref s) => Some(s.as_str()),
            _ => None,
        };
        let rear_compound = match frame.get(ch::TYRE_COMPOUND_REAR).value {
            Value::Str(ref s) => Some(s.as_str()),
            _ => None,
        };

        let stint_sample = StintSample {
            et_s,
            pit_phase,
            fuel_l: frame_f64(frame, ch::FUEL),
            tyre_wear,
            front_compound,
            rear_compound,
        };
        let stint_events = self.stint.update(&stint_sample);

        if let Some(tyre_sample) = TyreSample::from_frame(frame) {
            self.tyre.update(&tyre_sample);
        }

        if let Some(brake_sample) = BrakeSample::from_frame(frame) {
            self.brake.update(&brake_sample);
        }

        // ---------------------------------------------------------------------
        // Step 6: Snapshots
        // ---------------------------------------------------------------------
        let timing_snap = self.timing.snapshot();
        let race_snap = self.race.snapshot().cloned();
        let race_finish = race_snap
            .as_ref()
            .map(|s| s.race_finish)
            .unwrap_or(RaceFinish::Unknown);

        let fuel_ctx = FuelContext {
            race_finish,
            lap_progress: timing_snap.lap_progress.or(lap_progress),
            laps_until_pit: None,
            laps_until_stint_end: None,
        };
        let fuel_estimate = self.fuel.estimate(&fuel_ctx);
        let stint_snap = self.stint.snapshot();
        let energy_snap = self.energy.snapshot();
        let tyre_snap = self.tyre.snapshot();
        let brake_snap = self.brake.snapshot();
        let aids_snap = self.aids.snapshot();

        let max_laps = stint_max_laps(
            fuel_estimate.capacity.value,
            fuel_estimate.fuel_per_lap.value,
            energy_snap.ve_per_lap.value,
            Some(energy_snap.virtual_energy.is_available()),
        );

        // ---------------------------------------------------------------------
        // Step 7: Publish engine outputs to frame
        // ---------------------------------------------------------------------
        let outputs = EngineOutputs {
            timing: Some(&timing_snap),
            fuel: Some(&fuel_estimate),
            stint: Some(&stint_snap),
            stint_max_laps: Some(max_laps),
            race: race_snap.as_ref(),
        };

        let pub_report = publish(frame, &outputs);
        let energy_pub_report = publish_energy(frame, Some(&energy_snap));

        let extended_outputs = ExtendedOutputs {
            tyre: Some(&tyre_snap),
            brake: Some(&brake_snap),
            map_lap_distance: None,
            race: race_snap.as_ref(),
            aids: Some(&aids_snap),
        };
        let ext_pub_report = publish_extended(frame, &extended_outputs);

        let total_publish_errors = pub_report
            .errors
            .len()
            .saturating_add(energy_pub_report.errors.len())
            .saturating_add(ext_pub_report.errors.len());

        if total_publish_errors > 0 {
            self.publish_errors_count = self
                .publish_errors_count
                .saturating_add(total_publish_errors as u64);
            warn!(
                component = COMPONENT,
                errors = total_publish_errors,
                "publish reported channel contract or frame errors"
            );
        }

        // ---------------------------------------------------------------------
        // Step 8: Alerts evaluation
        // ---------------------------------------------------------------------
        let alert_clock =
            AlertClock::from_mono_ns(frame.capture_mono_ns, finite(frame.source_et_s));
        let alert_notices = self
            .alert
            .evaluate_frame(alert_clock, timing_snap.lap_number, frame);

        for notice in &alert_notices {
            match notice.state {
                AlertState::Raised => {
                    events.push(SessionEvent::AlertRaised {
                        alert_id: notice.alert.id.to_string(),
                        severity: notice.alert.severity,
                        message: notice.alert.message.clone(),
                    });
                }
                AlertState::Active => {
                    // Alert remains active, no new transition event.
                }
                AlertState::Cleared => {
                    events.push(SessionEvent::AlertCleared {
                        alert_id: notice.alert.id.to_string(),
                    });
                }
            }
        }

        debug!(
            component = COMPONENT,
            events_count = events.len(),
            alerts_count = alert_notices.len(),
            publish_errors = total_publish_errors,
            "processed frame through EngineHub"
        );

        EngineFrameOutcome {
            events,
            alert_notices,
            timing: timing_snap,
            fuel: fuel_estimate,
            stint: stint_snap,
            race: race_snap,
            energy: energy_snap,
            tyre: tyre_snap,
            brake: brake_snap,
            aids: aids_snap,
            stint_events,
            stint_max_laps: max_laps,
            publish_errors: total_publish_errors,
        }
    }
}

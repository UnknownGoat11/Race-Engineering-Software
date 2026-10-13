//! Scenario configuration: what the synthetic session contains (vehicle, rates, weather, cautions, pit
//! stops, invalid laps, source stalls, malformed values) plus ready-made presets for other crates' tests.

use telemetry_model::ch;
use telemetry_model::channel::ChannelId;
use telemetry_model::enums::{CompoundType, SessionType};
use telemetry_model::registry;

use crate::error::SyntheticError;
use crate::track::TrackSpec;
use crate::weather::temperature_bounds;
use crate::vehicle::VehicleKind;

/// Default telemetry rate (Hz), close to LMU's native physics output.
pub const DEFAULT_TELEMETRY_RATE_HZ: f64 = 100.0;
/// Default scoring rate (Hz), LMU's scoring update rate.
pub const DEFAULT_SCORING_RATE_HZ: f64 = 5.0;
/// Lowest telemetry rate: the update delta must stay within the registry's 0.5 s maximum.
pub const MIN_TELEMETRY_RATE_HZ: f64 = 2.0;
/// Highest telemetry rate.
pub const MAX_TELEMETRY_RATE_HZ: f64 = 1_000.0;
/// Most competitors besides the player.
pub const MAX_COMPETITORS: usize = 60;
/// Default number of competitors besides the player.
pub const DEFAULT_COMPETITORS: usize = 11;
/// Default session length (s): a one-hour timed race.
pub const DEFAULT_SESSION_S: f64 = 3_600.0;
/// Default start time of day (14:00, seconds since midnight).
pub const DEFAULT_START_TIME_OF_DAY_S: f64 = 14.0 * 3_600.0;
/// Seconds per day (time-of-day range).
const SECONDS_PER_DAY: f64 = 86_400.0;
/// Default lap-to-lap pace standard deviation (fraction of the lap time).
pub const DEFAULT_LAP_PACE_SIGMA: f64 = 0.004;
/// Default lap-to-lap fuel consumption standard deviation (fraction).
pub const DEFAULT_FUEL_SIGMA: f64 = 0.015;
/// Upper bound for the variability knobs (fractions).
const MAX_SIGMA: f64 = 0.2;

/// A full-course or local caution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CautionKind {
    /// Full-course yellow: speed capped (80 km/h), yellow flag state machine runs.
    FullCourseYellow,
    /// Safety car: speed capped (120 km/h), yellow flag state machine runs.
    SafetyCar,
    /// Local yellow in one sector (zero-based sector index); cars in it are "under yellow".
    LocalYellow { sector: u8 },
}

/// One caution period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CautionSpec {
    pub kind: CautionKind,
    /// Session time the caution starts (s).
    pub start_et_s: f64,
    pub duration_s: f64,
}

/// One player pit stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PitStopSpec {
    /// The car enters the pit lane at the end of this lap (1-based); the stop is on the next lap.
    pub lap: u32,
    /// Fuel to add (L); `None` fills the tank.
    pub refuel_l: Option<f64>,
    pub change_tyres: bool,
    /// New compound when changing tyres; `None` keeps the current one.
    pub compound: Option<CompoundType>,
}

/// A rain shower: linear ramp up, plateau, linear ramp down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RainSpec {
    pub start_et_s: f64,
    pub ramp_s: f64,
    /// Plateau length (s).
    pub duration_s: f64,
    /// Peak rain intensity (0..1).
    pub intensity: f64,
}

/// Ambient conditions and their drift.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeatherSpec {
    pub air_temp_c: f64,
    /// Track temperature above air in full sun (°C).
    pub track_temp_offset_c: f64,
    /// Linear air-temperature drift (°C per hour).
    pub drift_c_per_hour: f64,
    pub rain: Option<RainSpec>,
}

/// How a stalled source looks to the reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleMode {
    /// No frames at all (the reader's data event never fires).
    NoFrames,
    /// The same frame content and source ET over and over (frozen shared memory).
    FrozenFrames,
}

/// A window during which the source stalls. Session time is frozen during the stall.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaleSpec {
    /// Session time at which the stall begins (s).
    pub start_et_s: f64,
    /// Stall length in provider (wall) time (s).
    pub duration_s: f64,
    pub mode: StaleMode,
}

/// Kind of malformed value injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MalformedKind {
    /// NaN read from the source: the frame carries `Invalid`, the wire helper can encode the raw NaN.
    NaN,
    /// ±infinity, handled like NaN.
    Infinity,
    /// A finite value far outside the registry's plausible range, written as `Valid` for validators to catch.
    OutOfRange,
    /// A one-sample jump inside the range (a rate violation for rate-limited channels), written as `Valid`.
    Spike,
}

/// Random malformed-value injection.
#[derive(Debug, Clone, PartialEq)]
pub struct MalformedSpec {
    /// Probability that a frame carries one injection.
    pub probability: f64,
    pub kinds: Vec<MalformedKind>,
    /// Float channels with a registry range that may be corrupted.
    pub channels: Vec<ChannelId>,
    /// Injection window in session time (s).
    pub start_et_s: f64,
    pub end_et_s: f64,
}

impl MalformedSpec {
    /// Every kind, on a default channel set, during the whole session.
    pub fn every_kind(probability: f64) -> Self {
        Self {
            probability,
            kinds: vec![
                MalformedKind::NaN,
                MalformedKind::Infinity,
                MalformedKind::OutOfRange,
                MalformedKind::Spike,
            ],
            channels: vec![
                ch::SPEED,
                ch::RPM,
                ch::THROTTLE,
                ch::FUEL,
                ch::TYRE_TEMP_MIDDLE_FL,
                ch::BRAKE_TEMP_FR,
                ch::TYRE_PRESSURE_RL,
                ch::WATER_TEMP,
            ],
            start_et_s: 0.0,
            end_et_s: f64::INFINITY,
        }
    }
}

/// Session parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionSpec {
    pub session_type: SessionType,
    /// Timed session length (s).
    pub duration_s: f64,
    pub start_time_of_day_s: f64,
}

/// Initial car state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StartSpec {
    /// Starting fuel (L); `None` = full tank.
    pub fuel_l: Option<f64>,
    /// Initial tyre carcass temperature (°C); cold tyres warm up over the first laps.
    pub tyre_temp_c: f64,
    pub virtual_energy_frac: f64,
    pub battery_soc_frac: f64,
}

/// Driver variability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriverSpec {
    /// Lap-to-lap pace standard deviation (fraction of the lap time).
    pub lap_pace_sigma: f64,
    /// Lap-to-lap fuel use standard deviation (fraction).
    pub fuel_sigma: f64,
}

/// A complete scenario. Build one with [`Scenario::builder`] or a preset.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub name: String,
    pub seed: u64,
    pub telemetry_rate_hz: f64,
    pub scoring_rate_hz: f64,
    pub vehicle: VehicleKind,
    pub track: TrackSpec,
    pub session: SessionSpec,
    /// Competitors besides the player.
    pub competitors: usize,
    /// Player's starting place (1-based).
    pub player_start_place: u8,
    pub start: StartSpec,
    pub driver: DriverSpec,
    pub weather: WeatherSpec,
    pub cautions: Vec<CautionSpec>,
    pub pit_stops: Vec<PitStopSpec>,
    /// Laps (1-based) on which the player exceeds track limits and the lap is invalidated.
    pub invalid_laps: Vec<u32>,
    pub stale: Vec<StaleSpec>,
    pub malformed: Option<MalformedSpec>,
}

impl Scenario {
    /// Builder with defaults: hybrid Hypercar, 100 Hz telemetry, 5 Hz scoring, dry, 11 competitors.
    pub fn builder(seed: u64) -> ScenarioBuilder {
        ScenarioBuilder {
            scenario: Scenario {
                name: "custom".to_owned(),
                seed,
                telemetry_rate_hz: DEFAULT_TELEMETRY_RATE_HZ,
                scoring_rate_hz: DEFAULT_SCORING_RATE_HZ,
                vehicle: VehicleKind::HybridHypercar,
                track: TrackSpec::synthetic_circuit(),
                session: SessionSpec {
                    session_type: SessionType::Race(1),
                    duration_s: DEFAULT_SESSION_S,
                    start_time_of_day_s: DEFAULT_START_TIME_OF_DAY_S,
                },
                competitors: DEFAULT_COMPETITORS,
                player_start_place: 4,
                start: StartSpec {
                    fuel_l: None,
                    tyre_temp_c: 35.0,
                    virtual_energy_frac: 1.0,
                    battery_soc_frac: 0.6,
                },
                driver: DriverSpec {
                    lap_pace_sigma: DEFAULT_LAP_PACE_SIGMA,
                    fuel_sigma: DEFAULT_FUEL_SIGMA,
                },
                weather: WeatherSpec {
                    air_temp_c: 22.0,
                    track_temp_offset_c: 12.0,
                    drift_c_per_hour: -1.5,
                    rain: None,
                },
                cautions: Vec::new(),
                pit_stops: Vec::new(),
                invalid_laps: Vec::new(),
                stale: Vec::new(),
                malformed: None,
            },
        }
    }

    /// Plain green-flag running in the dry.
    pub fn normal_lap(seed: u64) -> Self {
        Self::preset(Self::builder(seed).name("normal_lap"))
    }

    /// Pit stop at the end of lap 1 (refuel to full, four new tyres).
    pub fn pit_lap(seed: u64) -> Self {
        Self::preset(Self::builder(seed).name("pit_lap").pit_stop(PitStopSpec {
            lap: 1,
            refuel_l: None,
            change_tyres: true,
            compound: None,
        }))
    }

    /// Lap 2 is invalidated by a track-limits excursion.
    pub fn invalid_lap(seed: u64) -> Self {
        Self::preset(Self::builder(seed).name("invalid_lap").invalid_lap(2))
    }

    /// A shower from 30 s: 60 s ramp, 10 min plateau at 80 % intensity.
    pub fn rain(seed: u64) -> Self {
        Self::preset(Self::builder(seed).name("rain").rain(RainSpec {
            start_et_s: 30.0,
            ramp_s: 60.0,
            duration_s: 600.0,
            intensity: 0.8,
        }))
    }

    /// Hybrid Hypercar (motor, battery, regen channels valid).
    pub fn hybrid(seed: u64) -> Self {
        Self::preset(
            Self::builder(seed)
                .name("hybrid")
                .vehicle(VehicleKind::HybridHypercar),
        )
    }

    /// LMGT3 without hybrid system (hybrid channels NOT_SUPPORTED).
    pub fn non_hybrid(seed: u64) -> Self {
        Self::preset(
            Self::builder(seed)
                .name("non_hybrid")
                .vehicle(VehicleKind::Gt3),
        )
    }

    /// The source stalls twice: frozen frames for 3 s at 20 s, no frames for 2 s at 40 s.
    pub fn stale(seed: u64) -> Self {
        Self::preset(
            Self::builder(seed)
                .name("stale")
                .stale(StaleSpec {
                    start_et_s: 20.0,
                    duration_s: 3.0,
                    mode: StaleMode::FrozenFrames,
                })
                .stale(StaleSpec {
                    start_et_s: 40.0,
                    duration_s: 2.0,
                    mode: StaleMode::NoFrames,
                }),
        )
    }

    /// Malformed values (NaN, ±inf, out of range, spikes) in about 2 % of frames.
    pub fn malformed(seed: u64) -> Self {
        Self::preset(
            Self::builder(seed)
                .name("malformed")
                .malformed(MalformedSpec::every_kind(0.02)),
        )
    }

    /// Safety car from 30 s to 150 s, then a local yellow in sector 2 from 200 s to 230 s.
    pub fn safety_car(seed: u64) -> Self {
        Self::preset(
            Self::builder(seed)
                .name("safety_car")
                .caution(CautionSpec {
                    kind: CautionKind::SafetyCar,
                    start_et_s: 30.0,
                    duration_s: 120.0,
                })
                .caution(CautionSpec {
                    kind: CautionKind::LocalYellow { sector: 1 },
                    start_et_s: 200.0,
                    duration_s: 30.0,
                }),
        )
    }

    /// A long, lively demo: cold tyres, a local yellow, a full-course yellow, rain mid-race and two stops
    /// (the second onto wets).
    pub fn demo(seed: u64) -> Self {
        Self::preset(
            Self::builder(seed)
                .name("demo")
                .competitors(15)
                .caution(CautionSpec {
                    kind: CautionKind::LocalYellow { sector: 2 },
                    start_et_s: 240.0,
                    duration_s: 25.0,
                })
                .caution(CautionSpec {
                    kind: CautionKind::FullCourseYellow,
                    start_et_s: 600.0,
                    duration_s: 150.0,
                })
                .rain(RainSpec {
                    start_et_s: 1_500.0,
                    ramp_s: 240.0,
                    duration_s: 900.0,
                    intensity: 0.7,
                })
                .pit_stop(PitStopSpec {
                    lap: 14,
                    refuel_l: None,
                    change_tyres: true,
                    compound: None,
                })
                .pit_stop(PitStopSpec {
                    lap: 20,
                    refuel_l: Some(20.0),
                    change_tyres: true,
                    compound: Some(CompoundType::Wet),
                })
                .invalid_lap(5),
        )
    }

    /// Presets are valid by construction; should one ever fail validation the defaults are returned and the
    /// problem is logged, so a preset never panics.
    fn preset(builder: ScenarioBuilder) -> Self {
        let fallback = builder.scenario.clone();
        match builder.build() {
            Ok(s) => s,
            Err(error) => {
                tracing::error!(component = "synthetic", %error, "invalid built-in scenario preset");
                fallback
            }
        }
    }

    /// Checks every parameter.
    pub fn validate(&self) -> Result<(), SyntheticError> {
        let rate = self.telemetry_rate_hz;
        if !(MIN_TELEMETRY_RATE_HZ..=MAX_TELEMETRY_RATE_HZ).contains(&rate) {
            return Err(SyntheticError::config(
                "telemetry_rate_hz",
                format!("{rate} outside {MIN_TELEMETRY_RATE_HZ}..={MAX_TELEMETRY_RATE_HZ}"),
            ));
        }
        if !(self.scoring_rate_hz > 0.0 && self.scoring_rate_hz <= rate) {
            return Err(SyntheticError::config(
                "scoring_rate_hz",
                "must be positive and at most the telemetry rate",
            ));
        }
        if self.competitors > MAX_COMPETITORS {
            return Err(SyntheticError::config(
                "competitors",
                format!("at most {MAX_COMPETITORS}"),
            ));
        }
        let field = usize::from(self.player_start_place);
        if field == 0 || field > self.competitors + 1 {
            return Err(SyntheticError::config(
                "player_start_place",
                "must be within 1..=competitors+1",
            ));
        }
        let s = &self.session;
        if !(s.duration_s.is_finite() && s.duration_s > 0.0)
            || !(0.0..SECONDS_PER_DAY).contains(&s.start_time_of_day_s)
        {
            return Err(SyntheticError::config(
                "session",
                "duration must be positive, start time of day within one day",
            ));
        }
        let st = &self.start;
        let fuel_ok = st.fuel_l.is_none_or(|f| f.is_finite() && f >= 0.0);
        if !fuel_ok
            || !(-20.0..=150.0).contains(&st.tyre_temp_c)
            || !(0.0..=1.0).contains(&st.virtual_energy_frac)
            || !(0.0..=1.0).contains(&st.battery_soc_frac)
        {
            return Err(SyntheticError::config(
                "start",
                "fuel ≥ 0, tyre temperature −20..150 °C, fractions within 0..1",
            ));
        }
        let d = &self.driver;
        if !(0.0..=MAX_SIGMA).contains(&d.lap_pace_sigma)
            || !(0.0..=MAX_SIGMA).contains(&d.fuel_sigma)
        {
            return Err(SyntheticError::config(
                "driver",
                format!("variability must be within 0..={MAX_SIGMA}"),
            ));
        }
        self.validate_weather()?;
        self.validate_events()?;
        if let Some(m) = &self.malformed {
            validate_malformed(m)?;
        }
        Ok(())
    }

    fn validate_weather(&self) -> Result<(), SyntheticError> {
        let w = &self.weather;
        let ok = (-20.0..=50.0).contains(&w.air_temp_c)
            && (0.0..=30.0).contains(&w.track_temp_offset_c)
            && w.drift_c_per_hour.is_finite()
            && w.drift_c_per_hour.abs() <= 10.0;
        if !ok {
            return Err(SyntheticError::config(
                "weather",
                "air −20..50 °C, track offset 0..30 °C, drift within ±10 °C/h",
            ));
        }
        if let Some(r) = &w.rain {
            let ok = non_negative(r.start_et_s)
                && non_negative(r.ramp_s)
                && non_negative(r.duration_s)
                && (0.0..=1.0).contains(&r.intensity);
            if !ok {
                return Err(SyntheticError::config(
                    "weather.rain",
                    "times must be ≥ 0, intensity within 0..1",
                ));
            }
        }
        // Everything the model can produce over the session must pass the registry's plausibility ranges.
        let bounds = temperature_bounds(w, self.session.duration_s);
        for (id, (low, high)) in [(ch::AIR_TEMP, bounds.air_c), (ch::TRACK_TEMP, bounds.track_c)] {
            let Some((min, max)) = registry::by_id(id).and_then(|def| def.valid_range) else {
                continue;
            };
            if low < min || high > max {
                return Err(SyntheticError::config(
                    "weather",
                    format!(
                        "channel 0x{:04X} would reach {low:.1}..{high:.1} °C over the session, outside \
                         {min}..{max} °C",
                        id.0
                    ),
                ));
            }
        }
        Ok(())
    }

    fn validate_events(&self) -> Result<(), SyntheticError> {
        for c in &self.cautions {
            let sector_ok = match c.kind {
                CautionKind::LocalYellow { sector } => sector < 3,
                CautionKind::FullCourseYellow | CautionKind::SafetyCar => true,
            };
            if !(sector_ok
                && non_negative(c.start_et_s)
                && c.duration_s.is_finite()
                && c.duration_s > 0.0)
            {
                return Err(SyntheticError::config(
                    "cautions",
                    format!("invalid caution {c:?}"),
                ));
            }
        }
        let mut last_lap = 0;
        for p in &self.pit_stops {
            let refuel_ok = p.refuel_l.is_none_or(|f| f.is_finite() && f >= 0.0);
            if p.lap <= last_lap || !refuel_ok {
                return Err(SyntheticError::config(
                    "pit_stops",
                    "laps must be ≥ 1 and strictly increasing, refuel ≥ 0",
                ));
            }
            last_lap = p.lap;
        }
        if self.invalid_laps.contains(&0) {
            return Err(SyntheticError::config("invalid_laps", "laps are 1-based"));
        }
        for s in &self.stale {
            if !(non_negative(s.start_et_s) && s.duration_s.is_finite() && s.duration_s > 0.0) {
                return Err(SyntheticError::config(
                    "stale",
                    format!("invalid stall {s:?}"),
                ));
            }
        }
        Ok(())
    }
}

fn non_negative(v: f64) -> bool {
    v.is_finite() && v >= 0.0
}

fn validate_malformed(m: &MalformedSpec) -> Result<(), SyntheticError> {
    if !(0.0..=1.0).contains(&m.probability) || m.kinds.is_empty() || m.channels.is_empty() {
        return Err(SyntheticError::config(
            "malformed",
            "probability within 0..1 and at least one kind and channel",
        ));
    }
    if !(non_negative(m.start_et_s) && m.end_et_s >= m.start_et_s) {
        return Err(SyntheticError::config(
            "malformed",
            "window must start at ≥ 0 and end after it starts",
        ));
    }
    for id in &m.channels {
        let usable =
            registry::by_id(*id).is_some_and(|def| def.is_float() && def.valid_range.is_some());
        if !usable {
            return Err(SyntheticError::config(
                "malformed.channels",
                format!(
                    "channel 0x{:04X} is not a float channel with a plausible range",
                    id.0
                ),
            ));
        }
    }
    Ok(())
}

/// Fluent [`Scenario`] builder.
#[derive(Debug, Clone)]
pub struct ScenarioBuilder {
    scenario: Scenario,
}

impl ScenarioBuilder {
    pub fn name(mut self, name: &str) -> Self {
        self.scenario.name = name.to_owned();
        self
    }
    pub fn telemetry_rate_hz(mut self, hz: f64) -> Self {
        self.scenario.telemetry_rate_hz = hz;
        self
    }
    pub fn scoring_rate_hz(mut self, hz: f64) -> Self {
        self.scenario.scoring_rate_hz = hz;
        self
    }
    pub fn vehicle(mut self, kind: VehicleKind) -> Self {
        self.scenario.vehicle = kind;
        self
    }
    pub fn track(mut self, spec: TrackSpec) -> Self {
        self.scenario.track = spec;
        self
    }
    pub fn session(mut self, session: SessionSpec) -> Self {
        self.scenario.session = session;
        self
    }
    pub fn competitors(mut self, count: usize) -> Self {
        self.scenario.competitors = count;
        self
    }
    pub fn player_start_place(mut self, place: u8) -> Self {
        self.scenario.player_start_place = place;
        self
    }
    pub fn start(mut self, start: StartSpec) -> Self {
        self.scenario.start = start;
        self
    }
    pub fn driver(mut self, driver: DriverSpec) -> Self {
        self.scenario.driver = driver;
        self
    }
    pub fn weather(mut self, weather: WeatherSpec) -> Self {
        self.scenario.weather = weather;
        self
    }
    pub fn rain(mut self, rain: RainSpec) -> Self {
        self.scenario.weather.rain = Some(rain);
        self
    }
    pub fn caution(mut self, caution: CautionSpec) -> Self {
        self.scenario.cautions.push(caution);
        self
    }
    pub fn pit_stop(mut self, stop: PitStopSpec) -> Self {
        self.scenario.pit_stops.push(stop);
        self
    }
    pub fn invalid_lap(mut self, lap: u32) -> Self {
        self.scenario.invalid_laps.push(lap);
        self
    }
    pub fn stale(mut self, stall: StaleSpec) -> Self {
        self.scenario.stale.push(stall);
        self
    }
    pub fn malformed(mut self, spec: MalformedSpec) -> Self {
        self.scenario.malformed = Some(spec);
        self
    }

    /// Validates and returns the scenario.
    pub fn build(self) -> Result<Scenario, SyntheticError> {
        self.scenario.validate()?;
        Ok(self.scenario)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid() {
        for s in [
            Scenario::normal_lap(1),
            Scenario::pit_lap(1),
            Scenario::invalid_lap(1),
            Scenario::rain(1),
            Scenario::hybrid(1),
            Scenario::non_hybrid(1),
            Scenario::stale(1),
            Scenario::malformed(1),
            Scenario::safety_car(1),
            Scenario::demo(1),
        ] {
            assert!(s.validate().is_ok(), "{}", s.name);
            assert_ne!(s.name, "custom");
        }
    }

    #[test]
    fn invalid_parameters_are_rejected() {
        assert!(Scenario::builder(1).telemetry_rate_hz(1.0).build().is_err());
        assert!(
            Scenario::builder(1)
                .telemetry_rate_hz(5_000.0)
                .build()
                .is_err()
        );
        assert!(Scenario::builder(1).scoring_rate_hz(200.0).build().is_err());
        assert!(Scenario::builder(1).competitors(500).build().is_err());
        assert!(Scenario::builder(1).player_start_place(0).build().is_err());
        assert!(Scenario::builder(1).invalid_lap(0).build().is_err());
        let stop = PitStopSpec {
            lap: 2,
            refuel_l: None,
            change_tyres: false,
            compound: None,
        };
        assert!(
            Scenario::builder(1)
                .pit_stop(stop)
                .pit_stop(stop)
                .build()
                .is_err()
        );
        let mut m = MalformedSpec::every_kind(0.5);
        m.channels.push(ch::GEAR);
        assert!(Scenario::builder(1).malformed(m).build().is_err());
        assert!(
            Scenario::builder(1)
                .caution(CautionSpec {
                    kind: CautionKind::LocalYellow { sector: 3 },
                    start_et_s: 1.0,
                    duration_s: 1.0
                })
                .build()
                .is_err()
        );
    }

    /// Regression (review pass 2, F5): drift × session time was not checked, so a valid scenario published
    /// out-of-range air/track temperatures as Valid.
    #[test]
    fn temperatures_must_stay_in_range_over_the_session() {
        let cold = WeatherSpec {
            air_temp_c: -20.0,
            track_temp_offset_c: 0.0,
            drift_c_per_hour: -10.0,
            rain: None,
        };
        let session = |duration_s| SessionSpec {
            duration_s,
            ..Scenario::builder(1).build().expect("default").session
        };
        let err = Scenario::builder(1)
            .weather(cold)
            .session(session(3_600.0))
            .build();
        assert!(
            matches!(
                err,
                Err(SyntheticError::InvalidConfig {
                    field: "weather",
                    ..
                })
            ),
            "{err:?}"
        );
        // The same drift over a short session stays in range.
        assert!(
            Scenario::builder(1)
                .weather(cold)
                .session(session(600.0))
                .build()
                .is_ok()
        );
        // Hot air with a large solar offset exceeds the track-temperature range.
        let hot = WeatherSpec {
            air_temp_c: 50.0,
            track_temp_offset_c: 30.0,
            drift_c_per_hour: 0.0,
            rain: None,
        };
        assert!(Scenario::builder(1).weather(hot).build().is_err());
    }
}

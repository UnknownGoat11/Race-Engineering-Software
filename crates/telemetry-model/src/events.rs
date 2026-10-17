//! Session events: discrete things that happened (laps, pits, flags, connection, recording, alerts).
//!
//! An [`EventRecord`] wraps a [`SessionEvent`] with its timestamp (source ET + driver monotonic ns), lap,
//! category, severity and a human description. Category, severity and description are derived from the
//! event by [`EventRecord::new`] (deterministic) and stored so consumers (UI, logs, recordings) need no
//! logic. Serde structs, postcard-friendly.

use serde::{Deserialize, Serialize};

use crate::enums::{GamePhase, SessionType, YellowFlagState};

/// When an event happened.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EventTime {
    /// LMU elapsed session time, `None` when no LMU time applies (e.g. network events).
    pub source_et_s: Option<f64>,
    /// Driver (or engineer, for local events) monotonic nanoseconds.
    pub mono_ns: u64,
}

/// Event grouping for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventCategory {
    Source,
    Lap,
    Pit,
    Flag,
    Session,
    Weather,
    Schema,
    Network,
    Recording,
    Alert,
    Performance,
}

/// Event importance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Notice,
    Warning,
    Critical,
}

/// Race-control flag summary shown to the user (derived from game phase, yellow state and car flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RaceFlag {
    Green,
    /// Local (sector) yellow.
    Yellow,
    /// Full-course yellow / safety car.
    FullCourseYellow,
    Blue,
    /// Session stopped / race halted.
    Red,
    Chequered,
    /// No flag information.
    None,
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SessionEvent {
    /// The LMU shared memory became readable.
    LmuConnected {
        game_version: Option<i32>,
    },
    /// The LMU shared memory went away (game closed, mapping lost).
    LmuDisconnected {
        reason: String,
    },
    /// LMU paused (options UI, monitor) or resumed driving.
    LmuPaused,
    LmuResumed,
    LapStarted {
        lap: u32,
    },
    /// A lap finished; `time_s` is `None` when LMU reported no time; `valid` is false for invalidated laps.
    LapCompleted {
        lap: u32,
        time_s: Option<f64>,
        valid: bool,
    },
    /// Sector 1..=3 finished.
    SectorCompleted {
        lap: u32,
        sector: u8,
        time_s: Option<f64>,
    },
    PitEntry,
    PitExit,
    /// A completed stop; `stationary_s` is the time stopped in the box when known.
    PitStop {
        stationary_s: Option<f64>,
    },
    RaceFlagChanged {
        from: RaceFlag,
        to: RaceFlag,
    },
    GamePhaseChanged {
        from: Option<GamePhase>,
        to: GamePhase,
    },
    YellowFlagStateChanged {
        from: Option<YellowFlagState>,
        to: YellowFlagState,
    },
    SessionChanged {
        from: Option<SessionType>,
        to: SessionType,
    },
    VehicleChanged {
        vehicle_name: Option<String>,
        class_name: Option<String>,
    },
    TrackChanged {
        track_name: Option<String>,
    },
    /// Significant weather change (thresholds decided by the producer).
    WeatherChanged {
        raining_frac: Option<f64>,
        path_wetness_avg_frac: Option<f64>,
        ambient_temp_c: Option<f64>,
        track_temp_c: Option<f64>,
    },
    PenaltyReceived {
        total_penalties: u32,
    },
    TelemetrySchemaChanged {
        from: u16,
        to: u16,
    },
    NetworkConnected {
        peer: String,
    },
    NetworkDisconnected {
        reason: String,
    },
    /// Link quality crossed a degradation threshold.
    NetworkDegraded {
        reason: String,
        packet_loss_frac: Option<f64>,
        latency_s: Option<f64>,
    },
    NetworkRecovered,
    RecordingStarted {
        recording_id: String,
    },
    RecordingStopped {
        recording_id: String,
        frames: u64,
        bytes: u64,
    },
    RecordingFailed {
        recording_id: Option<String>,
        reason: String,
    },
    AlertRaised {
        alert_id: String,
        severity: Severity,
        message: String,
    },
    AlertCleared {
        alert_id: String,
    },
    /// New personal best lap for the player.
    PersonalBest {
        lap: u32,
        time_s: f64,
    },
    /// New best lap of the session (overall or within the class).
    SessionBest {
        lap: u32,
        time_s: f64,
        slot_id: i32,
        class_only: bool,
    },
}

impl SessionEvent {
    /// Category of this event.
    pub fn category(&self) -> EventCategory {
        use SessionEvent as E;
        match self {
            E::LmuConnected { .. } | E::LmuDisconnected { .. } | E::LmuPaused | E::LmuResumed => {
                EventCategory::Source
            }
            E::LapStarted { .. } | E::LapCompleted { .. } | E::SectorCompleted { .. } => {
                EventCategory::Lap
            }
            E::PitEntry | E::PitExit | E::PitStop { .. } => EventCategory::Pit,
            E::RaceFlagChanged { .. }
            | E::GamePhaseChanged { .. }
            | E::YellowFlagStateChanged { .. }
            | E::PenaltyReceived { .. } => EventCategory::Flag,
            E::SessionChanged { .. } | E::VehicleChanged { .. } | E::TrackChanged { .. } => {
                EventCategory::Session
            }
            E::WeatherChanged { .. } => EventCategory::Weather,
            E::TelemetrySchemaChanged { .. } => EventCategory::Schema,
            E::NetworkConnected { .. }
            | E::NetworkDisconnected { .. }
            | E::NetworkDegraded { .. }
            | E::NetworkRecovered => EventCategory::Network,
            E::RecordingStarted { .. } | E::RecordingStopped { .. } | E::RecordingFailed { .. } => {
                EventCategory::Recording
            }
            E::AlertRaised { .. } | E::AlertCleared { .. } => EventCategory::Alert,
            E::PersonalBest { .. } | E::SessionBest { .. } => EventCategory::Performance,
        }
    }

    /// Default severity of this event.
    pub fn severity(&self) -> Severity {
        use SessionEvent as E;
        match self {
            E::AlertRaised { severity, .. } => *severity,
            E::LmuDisconnected { .. }
            | E::NetworkDisconnected { .. }
            | E::NetworkDegraded { .. } => Severity::Warning,
            E::RecordingFailed { .. } => Severity::Critical,
            E::RaceFlagChanged {
                to: RaceFlag::Red, ..
            } => Severity::Critical,
            E::RaceFlagChanged {
                to: RaceFlag::FullCourseYellow | RaceFlag::Yellow | RaceFlag::Blue,
                ..
            } => Severity::Warning,
            E::PenaltyReceived { .. } | E::TelemetrySchemaChanged { .. } => Severity::Warning,
            E::LapCompleted { valid: false, .. } => Severity::Notice,
            E::PersonalBest { .. }
            | E::SessionBest { .. }
            | E::PitStop { .. }
            | E::RaceFlagChanged { .. } => Severity::Notice,
            E::SessionChanged { .. } | E::VehicleChanged { .. } | E::TrackChanged { .. } => {
                Severity::Notice
            }
            _ => Severity::Info,
        }
    }

    /// Short human-readable description.
    pub fn description(&self) -> String {
        use SessionEvent as E;
        match self {
            E::LmuConnected {
                game_version: Some(v),
            } => format!("Connected to LMU (game version {v})"),
            E::LmuConnected { game_version: None } => "Connected to LMU".to_owned(),
            E::LmuDisconnected { reason } => format!("LMU disconnected: {reason}"),
            E::LmuPaused => "LMU paused".to_owned(),
            E::LmuResumed => "LMU resumed".to_owned(),
            E::LapStarted { lap } => format!("Lap {lap} started"),
            E::LapCompleted { lap, time_s, valid } => {
                let time = time_s.map_or_else(|| "no time".to_owned(), fmt_lap_time);
                let validity = if *valid { "" } else { " (invalid)" };
                format!("Lap {lap} completed: {time}{validity}")
            }
            E::SectorCompleted {
                lap,
                sector,
                time_s,
            } => {
                let time = time_s.map_or_else(|| "no time".to_owned(), |t| format!("{t:.3} s"));
                format!("Lap {lap} sector {sector}: {time}")
            }
            E::PitEntry => "Pit entry".to_owned(),
            E::PitExit => "Pit exit".to_owned(),
            E::PitStop {
                stationary_s: Some(t),
            } => format!("Pit stop ({t:.1} s stationary)"),
            E::PitStop { stationary_s: None } => "Pit stop".to_owned(),
            E::RaceFlagChanged { from, to } => format!("Flag {from:?} → {to:?}"),
            E::GamePhaseChanged { from, to } => match from {
                Some(f) => format!("Game phase {f} → {to}"),
                None => format!("Game phase {to}"),
            },
            E::YellowFlagStateChanged { from, to } => match from {
                Some(f) => format!("Full-course yellow {f} → {to}"),
                None => format!("Full-course yellow {to}"),
            },
            E::SessionChanged { to, .. } => format!("Session: {to}"),
            E::VehicleChanged {
                vehicle_name,
                class_name,
            } => format!(
                "Vehicle: {} ({})",
                vehicle_name.as_deref().unwrap_or("unknown"),
                class_name.as_deref().unwrap_or("unknown class")
            ),
            E::TrackChanged { track_name } => {
                format!("Track: {}", track_name.as_deref().unwrap_or("unknown"))
            }
            E::WeatherChanged {
                raining_frac,
                path_wetness_avg_frac,
                ..
            } => format!(
                "Weather changed (rain {}, track wetness {})",
                fmt_pct(*raining_frac),
                fmt_pct(*path_wetness_avg_frac)
            ),
            E::PenaltyReceived { total_penalties } => {
                format!("Penalty received ({total_penalties} total)")
            }
            E::TelemetrySchemaChanged { from, to } => format!("Telemetry schema {from} → {to}"),
            E::NetworkConnected { peer } => format!("Connected to {peer}"),
            E::NetworkDisconnected { reason } => format!("Network disconnected: {reason}"),
            E::NetworkDegraded { reason, .. } => format!("Network degraded: {reason}"),
            E::NetworkRecovered => "Network recovered".to_owned(),
            E::RecordingStarted { recording_id } => format!("Recording {recording_id} started"),
            E::RecordingStopped {
                recording_id,
                frames,
                ..
            } => {
                format!("Recording {recording_id} stopped ({frames} frames)")
            }
            E::RecordingFailed { reason, .. } => format!("Recording failed: {reason}"),
            E::AlertRaised { message, .. } => message.clone(),
            E::AlertCleared { alert_id } => format!("Alert {alert_id} cleared"),
            E::PersonalBest { lap, time_s } => {
                format!("Personal best on lap {lap}: {}", fmt_lap_time(*time_s))
            }
            E::SessionBest {
                lap,
                time_s,
                class_only,
                ..
            } => {
                let scope = if *class_only { "Class" } else { "Session" };
                format!("{scope} best on lap {lap}: {}", fmt_lap_time(*time_s))
            }
        }
    }
}

/// Milliseconds per second and per minute (lap time formatting).
const MS_PER_S: u64 = 1_000;
const MS_PER_MINUTE: u64 = 60_000;
/// Fraction → percent.
const PERCENT: f64 = 100.0;

/// `m:ss.mmm`, rounded to the millisecond first so 59.9996 s renders as `1:00.000`, never `0:60.000`.
fn fmt_lap_time(t: f64) -> String {
    if !t.is_finite() || t < 0.0 {
        return "invalid time".to_owned();
    }
    // Saturating float → int cast; lap times are far below u64::MAX ms.
    let total_ms = (t * MS_PER_S as f64).round() as u64;
    let minutes = total_ms / MS_PER_MINUTE;
    let rem_ms = total_ms % MS_PER_MINUTE;
    format!(
        "{minutes}:{:02}.{:03}",
        rem_ms / MS_PER_S,
        rem_ms % MS_PER_S
    )
}

fn fmt_pct(v: Option<f64>) -> String {
    v.map_or_else(|| "unknown".to_owned(), |f| format!("{:.0} %", f * PERCENT))
}

/// A timestamped event with its derived classification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventRecord {
    pub at: EventTime,
    /// Player lap number when the event happened, if known.
    pub lap: Option<u32>,
    pub category: EventCategory,
    pub severity: Severity,
    pub description: String,
    pub event: SessionEvent,
}

impl EventRecord {
    /// Builds a record, deriving category, severity and description from `event`.
    pub fn new(at: EventTime, lap: Option<u32>, event: SessionEvent) -> Self {
        Self {
            at,
            lap,
            category: event.category(),
            severity: event.severity(),
            description: event.description(),
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> EventTime {
        EventTime {
            source_et_s: Some(812.5),
            mono_ns: 42,
        }
    }

    #[test]
    fn records_derive_classification() {
        let r = EventRecord::new(
            at(),
            Some(12),
            SessionEvent::LapCompleted {
                lap: 12,
                time_s: Some(225.123),
                valid: true,
            },
        );
        assert_eq!(r.category, EventCategory::Lap);
        assert_eq!(r.severity, Severity::Info);
        assert_eq!(r.description, "Lap 12 completed: 3:45.123");

        let r = EventRecord::new(
            at(),
            None,
            SessionEvent::RecordingFailed {
                recording_id: None,
                reason: "disk full".into(),
            },
        );
        assert_eq!(
            (r.category, r.severity),
            (EventCategory::Recording, Severity::Critical)
        );

        let alert = SessionEvent::AlertRaised {
            alert_id: "fuel".into(),
            severity: Severity::Warning,
            message: "Box this lap".into(),
        };
        assert_eq!(alert.severity(), Severity::Warning);
        assert_eq!(alert.description(), "Box this lap");

        let flag = SessionEvent::RaceFlagChanged {
            from: RaceFlag::Green,
            to: RaceFlag::Red,
        };
        assert_eq!(
            (flag.category(), flag.severity()),
            (EventCategory::Flag, Severity::Critical)
        );
        assert_eq!(
            SessionEvent::LapCompleted {
                lap: 1,
                time_s: None,
                valid: false
            }
            .severity(),
            Severity::Notice
        );
    }

    #[test]
    fn lap_time_formatting() {
        assert_eq!(fmt_lap_time(59.9994), "0:59.999");
        assert_eq!(fmt_lap_time(61.5), "1:01.500");
        assert_eq!(fmt_lap_time(59.9996), "1:00.000");
        assert_eq!(fmt_lap_time(f64::NAN), "invalid time");
    }

    #[test]
    fn postcard_roundtrip() {
        let records = vec![
            EventRecord::new(
                at(),
                Some(3),
                SessionEvent::PitStop {
                    stationary_s: Some(31.2),
                },
            ),
            EventRecord::new(
                at(),
                None,
                SessionEvent::SessionChanged {
                    from: None,
                    to: SessionType::from_lmu(10),
                },
            ),
            EventRecord::new(
                at(),
                None,
                SessionEvent::GamePhaseChanged {
                    from: None,
                    to: GamePhase::from_raw(77),
                },
            ),
        ];
        let bytes = postcard::to_stdvec(&records).expect("serialize");
        let back: Vec<EventRecord> = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(back, records);
    }
}

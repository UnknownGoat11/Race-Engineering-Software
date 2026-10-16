//! Identifiers, statuses and index row types shared by the writer, index, recovery and reader.

use std::fmt;
use std::str::FromStr;

use uuid::Uuid;

use crate::format::MAX_APP_VERSION_BYTES;

/// Identifier of one recording session (also written into every segment file header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// A new random (v4) session id.
    pub fn new_random() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

impl FromStr for SessionId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

/// Defines a closed set of upper-case string codes for an enum stored as TEXT in the index.
macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            /// Stable upper-case code stored in the index.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            /// Parses a code produced by [`Self::as_str`].
            pub fn parse(text: &str) -> Option<Self> {
                match text {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

text_enum! {
    /// Lifecycle status of a session.
    SessionStatus {
        /// Being recorded (or the application died while recording and recovery has not run yet).
        Active => "ACTIVE",
        /// Finished normally and validated.
        Complete => "COMPLETE",
        /// The application stopped without finishing the session; recovered by the startup scan.
        Interrupted => "INTERRUPTED",
    }
}

text_enum! {
    /// Result of byte-level integrity validation (independent of dropped records, which are counted
    /// separately and noted in the recording).
    IntegrityStatus {
        /// Not validated yet (session or segment still being written).
        Unverified => "UNVERIFIED",
        /// Every byte validated (chunk CRCs, decompression, framing; whole-file CRC matches).
        Ok => "OK",
        /// Valid data followed by an incomplete final write (crash), or a segment created but never written;
        /// nothing before it is affected.
        Truncated => "TRUNCATED",
        /// Corruption before the end of a segment, a missing segment, or a mismatch with what the writer
        /// recorded. The damaged chunks' records are lost; intact chunks after a damaged chunk body stay
        /// readable, but nothing after a damaged chunk header can be located.
        Damaged => "DAMAGED",
    }
}

impl IntegrityStatus {
    fn severity(self) -> u8 {
        match self {
            IntegrityStatus::Ok => 0,
            IntegrityStatus::Unverified => 1,
            IntegrityStatus::Truncated => 2,
            IntegrityStatus::Damaged => 3,
        }
    }

    /// The more severe of two statuses (used to combine per-segment results into a session result).
    pub fn worst(self, other: Self) -> Self {
        if other.severity() > self.severity() {
            other
        } else {
            self
        }
    }
}

text_enum! {
    /// Lifecycle status of one segment file.
    SegmentStatus {
        /// Being written.
        Open => "OPEN",
        /// Closed normally by the writer.
        Closed => "CLOSED",
        /// A write/sync failed; the writer abandoned the file.
        Failed => "FAILED",
        /// Scanned by startup recovery after an interruption.
        Recovered => "RECOVERED",
        /// Listed in the index but the file does not exist.
        Missing => "MISSING",
    }
}

text_enum! {
    /// Severity of an indexed event.
    Severity {
        Info => "INFO",
        Warning => "WARNING",
        Critical => "CRITICAL",
    }
}

text_enum! {
    /// Per-session retention class: the hook a retention policy must respect.
    RetentionClass {
        /// Eligible for an (opt-in) retention policy.
        Default => "DEFAULT",
        /// Never selected by any retention policy (e.g. pinned by the user).
        Keep => "KEEP",
    }
}

/// Category of an indexed event.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EventCategory {
    /// Engine/rule alert (channel, value and threshold usually set).
    Alert,
    Lap,
    Pit,
    Flag,
    Session,
    /// Recorder health: overruns, write failures, disk space, interruptions.
    Recorder,
    /// LMU source state (detected, lost, interface mismatch).
    Lmu,
    Network,
    /// User bookmark / note.
    User,
    /// Any other category (stored verbatim, upper-case by convention).
    Other(String),
}

impl EventCategory {
    /// Stable code stored in the index.
    pub fn as_str(&self) -> &str {
        match self {
            EventCategory::Alert => "ALERT",
            EventCategory::Lap => "LAP",
            EventCategory::Pit => "PIT",
            EventCategory::Flag => "FLAG",
            EventCategory::Session => "SESSION",
            EventCategory::Recorder => "RECORDER",
            EventCategory::Lmu => "LMU",
            EventCategory::Network => "NETWORK",
            EventCategory::User => "USER",
            EventCategory::Other(s) => s,
        }
    }

    /// Parses a stored code (unknown codes become [`EventCategory::Other`]).
    pub fn parse(text: &str) -> Self {
        match text {
            "ALERT" => EventCategory::Alert,
            "LAP" => EventCategory::Lap,
            "PIT" => EventCategory::Pit,
            "FLAG" => EventCategory::Flag,
            "SESSION" => EventCategory::Session,
            "RECORDER" => EventCategory::Recorder,
            "LMU" => EventCategory::Lmu,
            "NETWORK" => EventCategory::Network,
            "USER" => EventCategory::User,
            other => EventCategory::Other(other.to_owned()),
        }
    }
}

/// Session-level metadata stored in the index. Unknown values are `None`, never placeholders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMeta {
    /// Application version (also written into every segment header; ≤ 35 bytes).
    pub app_version: String,
    /// `telemetry_model::TELEMETRY_SCHEMA_VERSION` of the producer.
    pub telemetry_schema_version: u16,
    /// Wire protocol version the session was streamed with, if streamed.
    pub protocol_version: Option<u16>,
    /// LMU game version, as reported by the game.
    pub lmu_version: Option<String>,
    /// LMU shared-memory interface / layout fingerprint.
    pub interface_version: Option<String>,
    /// Version of the race-engines formulas that produced derived values and alerts.
    pub calculation_engine_version: Option<String>,
    /// Vehicle (class / entry) as reported by LMU.
    pub vehicle: Option<String>,
    /// Track (layout) as reported by LMU.
    pub track: Option<String>,
    /// LMU session type (practice, qualifying, race…).
    pub session_type: Option<String>,
    /// Channel availability / capability map as JSON (opaque to the recorder, must be valid JSON).
    pub data_availability_json: Option<String>,
}

impl SessionMeta {
    /// Checks the constraints the file format imposes.
    pub fn validate(&self) -> Result<(), String> {
        if self.app_version.is_empty() {
            return Err("app_version must not be empty".to_owned());
        }
        if self.app_version.len() > MAX_APP_VERSION_BYTES {
            return Err(format!(
                "app_version is {} bytes, maximum is {MAX_APP_VERSION_BYTES}",
                self.app_version.len()
            ));
        }
        if self.telemetry_schema_version == 0 {
            return Err(
                "telemetry_schema_version must be set (schema versions start at 1)".to_owned(),
            );
        }
        Ok(())
    }
}

/// Inclusive capture-time range `[start_ns, end_ns]` used to select records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeRange {
    /// First capture time included (ns).
    pub start_ns: u64,
    /// Last capture time included (ns).
    pub end_ns: u64,
}

impl TimeRange {
    /// Every capture time.
    pub const ALL: TimeRange = TimeRange {
        start_ns: 0,
        end_ns: u64::MAX,
    };

    /// `[start_ns, end_ns]`; `None` when `start_ns > end_ns`.
    pub fn new(start_ns: u64, end_ns: u64) -> Option<Self> {
        (start_ns <= end_ns).then_some(Self { start_ns, end_ns })
    }

    /// Whether `ns` lies inside the range.
    pub fn contains(&self, ns: u64) -> bool {
        self.start_ns <= ns && ns <= self.end_ns
    }

    /// Whether `[min_ns, max_ns]` intersects the range.
    pub fn overlaps(&self, min_ns: u64, max_ns: u64) -> bool {
        min_ns <= self.end_ns && max_ns >= self.start_ns
    }
}

/// Partial metadata update (e.g. vehicle/track become known after the first scoring update). `None`
/// fields are left unchanged; each field has the meaning of the same [`SessionMeta`] field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMetaUpdate {
    /// See [`SessionMeta::protocol_version`].
    pub protocol_version: Option<u16>,
    /// See [`SessionMeta::lmu_version`].
    pub lmu_version: Option<String>,
    /// See [`SessionMeta::interface_version`].
    pub interface_version: Option<String>,
    /// See [`SessionMeta::calculation_engine_version`].
    pub calculation_engine_version: Option<String>,
    /// See [`SessionMeta::vehicle`].
    pub vehicle: Option<String>,
    /// See [`SessionMeta::track`].
    pub track: Option<String>,
    /// See [`SessionMeta::session_type`].
    pub session_type: Option<String>,
    /// See [`SessionMeta::data_availability_json`] (must be valid JSON).
    pub data_availability_json: Option<String>,
}

impl SessionMetaUpdate {
    /// Folds `later` into `self` (later values win).
    pub fn merge(&mut self, later: SessionMetaUpdate) {
        macro_rules! take {
            ($($f:ident),+) => { $(if later.$f.is_some() { self.$f = later.$f; })+ };
        }
        take!(
            protocol_version,
            lmu_version,
            interface_version,
            calculation_engine_version,
            vehicle,
            track,
            session_type,
            data_availability_json
        );
    }
}

/// One lap in the index. Times are capture-clock ns; unknown values are `None` (never 0).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LapRecord {
    /// Lap number as counted by LMU (key: writing the same number again updates the lap).
    pub lap_number: u32,
    /// Stint the lap belongs to, if known.
    pub stint_number: Option<u32>,
    /// Capture time at which the lap started.
    pub start_capture_ns: u64,
    /// Capture time at which the lap ended (`None` while in progress).
    pub end_capture_ns: Option<u64>,
    /// Lap time in seconds, if known.
    pub lap_time_s: Option<f64>,
    /// Sector times in seconds, if known.
    pub sector_times_s: [Option<f64>; 3],
    /// Whether the lap counts (track limits etc.), if known.
    pub valid: Option<bool>,
    /// Whether the lap included a pit stop, if known.
    pub pit_lap: Option<bool>,
    /// Derived per-lap metrics from race-engines as JSON (opaque to the recorder).
    pub metrics_json: Option<String>,
}

/// One stint in the index. Times are capture-clock ns; unknown values are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StintRecord {
    /// Stint number (key: writing the same number again updates the stint).
    pub stint_number: u32,
    /// Capture time at which the stint started.
    pub start_capture_ns: u64,
    /// Capture time at which the stint ended (`None` while in progress).
    pub end_capture_ns: Option<u64>,
    /// First lap of the stint, if known.
    pub start_lap: Option<u32>,
    /// Last lap of the stint, if known.
    pub end_lap: Option<u32>,
    /// Driver name, if known.
    pub driver: Option<String>,
    /// Derived per-stint metrics from race-engines as JSON (opaque to the recorder).
    pub metrics_json: Option<String>,
}

/// One event in the index (alerts, pit/flag events, recorder health…).
#[derive(Debug, Clone, PartialEq)]
pub struct EventRecord {
    /// Capture-clock time of the event. `None` when no capture time exists (e.g. a recorder-health event
    /// raised before any record arrived and without a capture clock): never a placeholder 0.
    pub capture_ns: Option<u64>,
    /// Wall clock of the event, if known.
    pub unix_ns: Option<i64>,
    /// Lap during which the event happened, if known.
    pub lap: Option<u32>,
    /// What kind of event this is.
    pub category: EventCategory,
    /// How serious it is.
    pub severity: Severity,
    /// Human-readable description.
    pub description: String,
    /// Channel id (`telemetry_model::ChannelId.0`) for alerts.
    pub channel_id: Option<u16>,
    /// Alert: the channel value that triggered it, in the channel's canonical (SI) unit.
    pub value: Option<f64>,
    /// Alert: the threshold it crossed, in the same unit as `value`.
    pub threshold: Option<f64>,
}

impl EventRecord {
    /// A plain event at capture time `capture_ns`, without alert fields.
    pub fn new(
        capture_ns: u64,
        category: EventCategory,
        severity: Severity,
        description: impl Into<String>,
    ) -> Self {
        Self {
            capture_ns: Some(capture_ns),
            unix_ns: None,
            lap: None,
            category,
            severity,
            description: description.into(),
            channel_id: None,
            value: None,
            threshold: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_codes_roundtrip() {
        for s in [
            SessionStatus::Active,
            SessionStatus::Complete,
            SessionStatus::Interrupted,
        ] {
            assert_eq!(SessionStatus::parse(s.as_str()), Some(s));
        }
        for s in [
            IntegrityStatus::Unverified,
            IntegrityStatus::Ok,
            IntegrityStatus::Truncated,
            IntegrityStatus::Damaged,
        ] {
            assert_eq!(IntegrityStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(
            SegmentStatus::parse("MISSING"),
            Some(SegmentStatus::Missing)
        );
        assert_eq!(SessionStatus::parse("active"), None);
        for c in [
            EventCategory::Alert,
            EventCategory::Recorder,
            EventCategory::Other("TYRES".into()),
        ] {
            assert_eq!(EventCategory::parse(c.as_str()), c);
        }
    }

    #[test]
    fn integrity_worst() {
        use IntegrityStatus::*;
        assert_eq!(Ok.worst(Truncated), Truncated);
        assert_eq!(Damaged.worst(Truncated), Damaged);
        assert_eq!(Ok.worst(Unverified), Unverified);
        assert_eq!(Ok.worst(Ok), Ok);
    }

    #[test]
    fn time_range_semantics() {
        let r = TimeRange::new(10, 20).expect("ordered");
        assert!(r.contains(10) && r.contains(20) && !r.contains(21) && !r.contains(9));
        assert!(
            r.overlaps(0, 10) && r.overlaps(20, 30) && r.overlaps(12, 13) && r.overlaps(0, 100)
        );
        assert!(!r.overlaps(0, 9) && !r.overlaps(21, 30));
        assert_eq!(TimeRange::new(5, 4), None);
        assert!(TimeRange::ALL.contains(0) && TimeRange::ALL.contains(u64::MAX));
    }

    #[test]
    fn meta_validation_and_merge() {
        let mut m = SessionMeta {
            app_version: "0.1.0".into(),
            telemetry_schema_version: 1,
            ..Default::default()
        };
        assert!(m.validate().is_ok());
        m.telemetry_schema_version = 0;
        assert!(m.validate().is_err(), "an unset schema version is rejected");
        m.telemetry_schema_version = 1;
        m.app_version = "x".repeat(MAX_APP_VERSION_BYTES + 1);
        assert!(m.validate().is_err());
        let mut u = SessionMetaUpdate {
            vehicle: Some("A".into()),
            track: Some("Spa".into()),
            ..Default::default()
        };
        u.merge(SessionMetaUpdate {
            vehicle: Some("B".into()),
            ..Default::default()
        });
        assert_eq!(u.vehicle.as_deref(), Some("B"));
        assert_eq!(u.track.as_deref(), Some("Spa"));
        let id = SessionId::new_random();
        assert_eq!(id.to_string().parse::<SessionId>().expect("parse"), id);
    }
}

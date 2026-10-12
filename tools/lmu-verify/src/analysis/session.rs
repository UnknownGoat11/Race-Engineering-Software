//! Session facts: game version and interface fingerprint, compatibility verdict, sync / lock modes,
//! player identification, session / track / vehicle segments, weather ranges, event codes and normalizer
//! diagnostics.
//!
//! Every string taken from the capture is redacted when the report is built ([`SessionAcc::finish`]).

use std::collections::{BTreeMap, BTreeSet};

use lmu_normalizer::NormalizedOutput;
use lmu_reader::compat::VERIFIED_GAME_VERSIONS;
use lmu_reader::offsets::{LAYOUT_FINGERPRINT, layout};
use lmu_reader::{
    CompatibilityReport, Consistency, PlayerIdentity, RawSnapshot, SharedMemoryEvent, SyncMode,
};
use serde::Serialize;
use telemetry_model::enums::{SessionType, VehicleClass};

use crate::redact::{NameCollector, Redactor};
use crate::stats::{Summary, SummaryStats};

/// Session / track / vehicle segments listed (later changes are only counted).
const MAX_SEGMENTS: usize = 50;
/// Distinct game versions remembered.
const MAX_VERSIONS: usize = 16;
/// Compatibility warnings / failures listed.
const MAX_COMPAT_LINES: usize = 40;
/// Distinct undocumented `generic.events[]` values listed (all of them are counted under "Other"): a
/// shifted generic block fills the slots with changing values, which must not grow the report unbounded.
const MAX_OTHER_EVENT_VALUES: usize = 16;
/// Key of the undocumented event values in [`SessionFacts::event_codes`].
const OTHER_EVENT_KEY: &str = "Other";

/// One stretch of the capture with the same session type, track and vehicle.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionSegment {
    /// Capture time of the first snapshot of the segment (s).
    pub start_capture_s: f64,
    /// Session type (`scoringInfo.mSession`, decoded).
    pub session_type: String,
    /// Track name (redacted).
    pub track: String,
    /// Player vehicle name (telemetry `mVehicleName`, redacted).
    pub vehicle: String,
    /// Telemetry `mVehicleClass` (the `IP_VehicleClass` enum).
    pub vehicle_class: String,
    /// Scoring class string of the player row (redacted).
    pub class_name: String,
    /// Snapshots in the segment.
    pub snapshots: u64,
}

/// Compatibility of the replayed frames.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompatSummary {
    /// `COMPATIBLE`, `COMPATIBLE_UNVERIFIED_VERSION`, `COMPATIBLE_WITH_WARNINGS` or `INCOMPATIBLE`.
    pub verdict: String,
    /// Overall support level of the replayed frames.
    pub overall: Option<String>,
    /// Frames the compatibility tracker checked.
    pub frames_checked: u64,
    /// Frames rejected by a structural check.
    pub frames_rejected: u64,
    /// Failed checks (`title: detected vs expected`).
    pub failures: Vec<String>,
    /// Warned checks.
    pub warnings: Vec<String>,
}

/// Everything in the "Session facts" section.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionFacts {
    /// Capture header lines (`key: value`, redacted).
    pub capture_meta: Vec<String>,
    /// Layout size the offsets of this build describe (bytes).
    pub layout_size: usize,
    /// Layout fingerprint of this build (the capture header must match it to be replayed).
    pub layout_fingerprint: String,
    /// `generic.gameVersion` values seen, with snapshot counts.
    pub game_versions: BTreeMap<i32, u64>,
    /// Versions listed as verified by this build (`lmu_reader::compat::VERIFIED_GAME_VERSIONS`).
    pub verified_versions: Vec<i32>,
    /// Snapshots whose game version is verified.
    pub version_verified_snapshots: u64,
    /// Compatibility of the replayed frames.
    pub compatibility: CompatSummary,
    /// Snapshots per frame synchronisation of the capturing reader.
    pub sync_modes: BTreeMap<String, u64>,
    /// Snapshots per copy consistency of the capturing reader.
    pub lock_modes: BTreeMap<String, u64>,
    /// Snapshots per player identification outcome.
    pub player_identity: BTreeMap<String, u64>,
    /// Session / track / vehicle segments (first 50).
    pub segments: Vec<SessionSegment>,
    /// Segment changes beyond the listed ones.
    pub segments_not_listed: u64,
    /// `scoringInfo.mAmbientTemp` at scoring updates (°C).
    pub ambient_temp_c: Option<SummaryStats>,
    /// `scoringInfo.mTrackTemp` at scoring updates (°C).
    pub track_temp_c: Option<SummaryStats>,
    /// Snapshots in realtime (`mInRealtime`).
    pub realtime_snapshots: u64,
    /// `generic.events[]` codes seen (per occurrence; the empty slot code is not counted). Every
    /// undocumented value is counted under one `Other` key.
    pub event_codes: BTreeMap<String, u64>,
    /// The first 16 distinct undocumented event values (sorted).
    pub other_event_values: Vec<u32>,
    /// Occurrences of undocumented event values beyond the listed ones.
    pub other_event_values_not_listed: u64,
    /// Normalizer diagnostic notices by code.
    pub notices: BTreeMap<String, u64>,
    /// Normalizer session events by kind.
    pub session_events: BTreeMap<String, u64>,
    /// Distinct personal names redacted from this report.
    pub names_redacted: usize,
    /// More names were seen than could be collected (redaction may be incomplete: do not share).
    pub names_overflowed: bool,
}

/// Identity of a segment before redaction.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SegmentKey {
    session_type: String,
    track: String,
    vehicle: String,
    vehicle_class: String,
    class_name: String,
}

/// Session facts accumulator.
#[derive(Debug, Clone, Default)]
pub struct SessionAcc {
    meta: String,
    versions: BTreeMap<i32, u64>,
    version_verified: u64,
    sync_modes: BTreeMap<&'static str, u64>,
    lock_modes: BTreeMap<&'static str, u64>,
    identity: BTreeMap<String, u64>,
    segments: Vec<(f64, SegmentKey, u64)>,
    current: Option<SegmentKey>,
    segments_not_listed: u64,
    ambient: Summary,
    track: Summary,
    realtime: u64,
    events: BTreeMap<String, u64>,
    other_events: BTreeSet<u32>,
    other_events_not_listed: u64,
    notices: BTreeMap<String, u64>,
    session_events: BTreeMap<String, u64>,
    names: NameCollector,
}

fn sync_label(sync: Option<SyncMode>) -> &'static str {
    match sync {
        Some(SyncMode::FrameGate) => "FRAME_GATE",
        Some(SyncMode::Polling) => "POLLING (degraded)",
        None => "UNKNOWN (not recorded)",
    }
}

fn consistency_label(c: Consistency) -> &'static str {
    match c {
        Consistency::Locked => "SDK_LOCK",
        Consistency::VerifiedLockFree { .. } => "LOCK_FREE (witness-verified)",
        Consistency::Replayed { locked: true } => "SDK_LOCK",
        Consistency::Replayed { locked: false } => "LOCK_FREE (witness-verified)",
    }
}

fn identity_label(p: PlayerIdentity) -> String {
    match p {
        PlayerIdentity::NoVehicle => "NO_VEHICLE".to_owned(),
        PlayerIdentity::Verified => "VERIFIED".to_owned(),
        PlayerIdentity::FoundById { .. } => "FOUND_BY_ID".to_owned(),
        PlayerIdentity::Unverified { reason } => format!("UNVERIFIED ({reason:?})"),
    }
}

impl SessionAcc {
    /// An accumulator for a capture with header text `meta`.
    pub fn new(meta: &str) -> Self {
        Self {
            meta: meta.to_owned(),
            ..Self::default()
        }
    }

    /// Feeds one snapshot and its normalizer output; `capture_s` is relative to the first snapshot.
    pub fn observe(&mut self, snapshot: &RawSnapshot, out: &NormalizedOutput, capture_s: f64) {
        let version = snapshot.generic.game_version;
        if self.versions.len() < MAX_VERSIONS || self.versions.contains_key(&version) {
            *self.versions.entry(version).or_insert(0) += 1;
        }
        if snapshot.version_verified {
            self.version_verified += 1;
        }
        *self
            .sync_modes
            .entry(sync_label(snapshot.source.sync))
            .or_insert(0) += 1;
        *self
            .lock_modes
            .entry(consistency_label(snapshot.source.consistency))
            .or_insert(0) += 1;
        *self
            .identity
            .entry(identity_label(snapshot.player_identity))
            .or_insert(0) += 1;
        let info = &snapshot.scoring.info;
        if info.in_realtime {
            self.realtime += 1;
        }
        for event in snapshot
            .events()
            .filter(|e| !SharedMemoryEvent::is_empty(*e))
        {
            self.observe_event(event);
        }
        if snapshot.scoring_updated || self.current.is_none() {
            self.ambient.push(info.ambient_temp);
            self.track.push(info.track_temp);
            self.collect_names(snapshot);
        }
        for notice in &out.notices {
            *self
                .notices
                .entry(format!("{:?}", notice.code))
                .or_insert(0) += 1;
        }
        for event in &out.events {
            let kind = format!("{:?}", event.event);
            let kind = kind
                .split(|c: char| !c.is_alphanumeric())
                .next()
                .unwrap_or("Unknown")
                .to_owned();
            *self.session_events.entry(kind).or_insert(0) += 1;
        }
        self.observe_segment(snapshot, capture_s);
    }

    /// Counts one non-empty event slot. Documented codes are a fixed, small set of keys; undocumented
    /// values share one key and only the first [`MAX_OTHER_EVENT_VALUES`] distinct ones are kept.
    fn observe_event(&mut self, event: SharedMemoryEvent) {
        let key = match event {
            SharedMemoryEvent::Other(raw) => {
                if self.other_events.len() < MAX_OTHER_EVENT_VALUES
                    || self.other_events.contains(&raw)
                {
                    self.other_events.insert(raw);
                } else {
                    self.other_events_not_listed += 1;
                }
                OTHER_EVENT_KEY.to_owned()
            }
            known => format!("{known:?}"),
        };
        *self.events.entry(key).or_insert(0) += 1;
    }

    fn collect_names(&mut self, snapshot: &RawSnapshot) {
        let info = &snapshot.scoring.info;
        self.names.observe(info.player_name.as_str());
        self.names.observe(info.plr_file_name.as_str());
        self.names.observe(info.server_name.as_str());
        for row in &snapshot.scoring.vehicles {
            self.names.observe(row.driver_name.as_str());
        }
    }

    fn observe_segment(&mut self, snapshot: &RawSnapshot, capture_s: f64) {
        let info = &snapshot.scoring.info;
        let player = snapshot.player.as_ref().map(|p| &p.telemetry);
        let key = SegmentKey {
            session_type: SessionType::from_lmu(info.session).to_string(),
            track: info.track_name.as_str().to_owned(),
            vehicle: player.map_or_else(String::new, |t| t.vehicle_name.as_str().to_owned()),
            vehicle_class: player.map_or_else(String::new, |t| {
                VehicleClass::from_raw(t.vehicle_class).to_string()
            }),
            class_name: snapshot
                .player_scoring()
                .map_or_else(String::new, |r| r.vehicle_class.as_str().to_owned()),
        };
        // A frame without player telemetry keeps the current segment (it says nothing about the car).
        if player.is_none() && self.current.is_some() {
            if let Some(last) = self.segments.last_mut() {
                last.2 += 1;
            }
            return;
        }
        if self.current.as_ref() == Some(&key) {
            if let Some(last) = self.segments.last_mut() {
                last.2 += 1;
            }
            return;
        }
        self.current = Some(key.clone());
        if self.segments.len() < MAX_SEGMENTS {
            self.segments.push((capture_s, key, 1));
        } else {
            self.segments_not_listed += 1;
        }
    }

    /// Names collected so far (for redacting other sections).
    pub fn names(&self) -> &NameCollector {
        &self.names
    }

    /// Builds the section; every capture string is redacted with `redactor`.
    pub fn finish(self, compat: &CompatibilityReport, redactor: &Redactor) -> SessionFacts {
        let capture_meta = self
            .meta
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| redactor.redact(l))
            .collect();
        SessionFacts {
            capture_meta,
            layout_size: layout::SIZE,
            layout_fingerprint: format!("{LAYOUT_FINGERPRINT:#018x}"),
            game_versions: self.versions,
            verified_versions: VERIFIED_GAME_VERSIONS.to_vec(),
            version_verified_snapshots: self.version_verified,
            compatibility: compat_summary(compat, redactor),
            sync_modes: owned_keys(self.sync_modes),
            lock_modes: owned_keys(self.lock_modes),
            player_identity: self.identity,
            segments: self
                .segments
                .into_iter()
                .map(|(start, k, n)| SessionSegment {
                    start_capture_s: start,
                    session_type: k.session_type,
                    track: redactor.redact(&k.track),
                    vehicle: redactor.redact(&k.vehicle),
                    vehicle_class: k.vehicle_class,
                    class_name: redactor.redact(&k.class_name),
                    snapshots: n,
                })
                .collect(),
            segments_not_listed: self.segments_not_listed,
            ambient_temp_c: self.ambient.stats(),
            track_temp_c: self.track.stats(),
            realtime_snapshots: self.realtime,
            event_codes: self.events,
            other_event_values: self.other_events.into_iter().collect(),
            other_event_values_not_listed: self.other_events_not_listed,
            notices: self.notices,
            session_events: self.session_events,
            names_redacted: self.names.len(),
            names_overflowed: self.names.overflowed(),
        }
    }
}

fn owned_keys(map: BTreeMap<&'static str, u64>) -> BTreeMap<String, u64> {
    map.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// Summarises a compatibility report (strings redacted).
pub fn compat_summary(compat: &CompatibilityReport, redactor: &Redactor) -> CompatSummary {
    let line = |c: &lmu_reader::CheckReport| {
        redactor.redact(&format!(
            "{}: detected {} (expected {}); warn frames {}, fail frames {}",
            c.title, c.detected, c.expected, c.warn_frames, c.fail_frames
        ))
    };
    let failures: Vec<String> = compat.failures().take(MAX_COMPAT_LINES).map(line).collect();
    let warnings: Vec<String> = compat.warnings().take(MAX_COMPAT_LINES).map(line).collect();
    let verdict = match (compat.overall, compat.version_verified) {
        (None, _) => "NO_FRAMES",
        (Some(lmu_reader::SupportLevel::Unsupported), _) => "INCOMPATIBLE",
        (Some(_), Some(false)) => "COMPATIBLE_UNVERIFIED_VERSION",
        (Some(lmu_reader::SupportLevel::SupportedWithWarnings), _) => "COMPATIBLE_WITH_WARNINGS",
        (Some(lmu_reader::SupportLevel::Supported), _) => "COMPATIBLE",
    };
    CompatSummary {
        verdict: verdict.to_owned(),
        overall: compat.overall.map(|o| o.as_str().to_owned()),
        frames_checked: compat.frames_checked,
        frames_rejected: compat.frames_rejected,
        failures,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undocumented_event_values_stay_bounded() {
        let mut acc = SessionAcc::default();
        acc.observe_event(SharedMemoryEvent::Enter);
        // A shifted generic block: a new value in every slot of every frame.
        for raw in 1_000..101_000u32 {
            acc.observe_event(SharedMemoryEvent::Other(raw));
        }
        // A listed value seen again is still counted as listed.
        acc.observe_event(SharedMemoryEvent::Other(1_000));
        assert_eq!(acc.events.len(), 2, "{:?}", acc.events);
        assert_eq!(acc.events.get(OTHER_EVENT_KEY), Some(&100_001));
        assert_eq!(acc.events.get("Enter"), Some(&1));
        assert_eq!(acc.other_events.len(), MAX_OTHER_EVENT_VALUES);
        assert_eq!(
            acc.other_events_not_listed,
            100_000 - MAX_OTHER_EVENT_VALUES as u64
        );
    }
}

//! Diagnostic notices: unit hazards, scale detections and source anomalies the normalizer found.
//!
//! Notices are diagnostics for the driver/engineer UI and the logs, not telemetry. Each (code, channel) pair
//! is raised at most once per session context ([`NoticeLog`]), so a persistent condition never floods the
//! output; the per-frame situation is visible in sample qualities and the capability map.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use telemetry_model::ChannelId;
use telemetry_model::events::Severity;

/// What a notice is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum NoticeCode {
    /// The brake temperature unit is not decided by evidence yet: brake temperatures are withheld
    /// (`Unavailable`), raw value attached (A-LMU-015).
    BrakeTempUnitUnverified,
    /// The brake temperature unit was decided from evidence.
    BrakeTempUnitDecided,
    /// Evidence for both Kelvin and Celsius was seen; the default hypothesis is kept.
    BrakeTempUnitConflict,
    /// `mBrakePressure` exceeded the fraction range: LMU probably switched to kPa (A-LMU-017). Mapping
    /// stopped for the session.
    BrakePressureUnitChanged,
    /// A fraction channel reads in percent (values above 1.5); divided by 100 from now on.
    FractionScalePercent,
    /// A fraction channel reads outside every known scale; published as INVALID, raw value in the notice.
    FractionScaleUnrecognized,
    /// A fraction channel's 0..1 scale was confirmed by evidence (A-LMU-013/014).
    FractionScaleFraction,
    /// A fraction channel's values fit both 0..1 and percent: withheld until evidence decides
    /// (A-LMU-013/014).
    FractionScaleUndecided,
    /// A fraction channel read a value that contradicts the scale verified for the running LMU version: it
    /// is withheld (`Unavailable`) for the session (A-LMU-013/014).
    FractionScaleConflict,
    /// A continuous channel kept its exact first value while the car drove: LMU does not fill it for this
    /// car/version, so it is withheld (`Unavailable`) until it changes (A-LMU-065).
    ChannelNotFilled,
    /// A channel was rejected by registry validation for many consecutive frames (sustained out-of-range
    /// or implausible value, e.g. a unit mix-up; A-LMU-049).
    ValueRejectedSustained,
    /// `mTurboBoostPressure` reads in kPa rather than Pa (A-LMU-027).
    TurboBoostKpa,
    /// A temperature without a header unit reads in Kelvin (A-LMU-038).
    TemperatureKelvinDetected,
    /// `mTimeOfDay` is not above 24 yet: hours vs seconds is undecided, the channel is withheld
    /// (A-LMU-036).
    TimeOfDayScaleUndecided,
    /// The player telemetry ET went backwards (session restart, A-LMU-031).
    EtRegression,
    /// The player telemetry ET is NaN or infinite: the sample was dropped.
    NonFiniteEt,
    /// The scoring snapshot has duplicate `mID`s or several player rows: standings dropped as SUSPECT.
    StandingsInconsistent,
    /// No scoring row is flagged `mIsPlayer` with the player's `mID`: player scoring channels unavailable.
    PlayerScoringRowMissing,
    /// `generic.gameVersion` is not a verified version: the layout is unconfirmed for this build.
    GameVersionUnverified,
    /// A value could not be written to the frame (internal mapping error, a bug).
    MappingError,
    /// `mSoC` (after scaling) disagrees with `mBatteryChargeFraction`: the battery fraction is published as
    /// the state of charge instead (A-LMU-014).
    HybridSocDisagreement,
}

/// One diagnostic notice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticNotice {
    pub code: NoticeCode,
    pub severity: Severity,
    /// The channel concerned, if any.
    pub channel: Option<ChannelId>,
    /// Human-readable explanation.
    pub message: String,
    /// Raw LMU value that triggered the notice, if any.
    pub raw_value: Option<f64>,
    /// LMU elapsed time of the triggering sample, if known.
    pub source_et_s: Option<f64>,
}

impl DiagnosticNotice {
    /// A notice without raw value or time.
    pub fn new(
        code: NoticeCode,
        severity: Severity,
        channel: Option<ChannelId>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity,
            channel,
            message: message.into(),
            raw_value: None,
            source_et_s: None,
        }
    }

    /// Attaches the raw value that triggered the notice.
    pub fn with_raw(mut self, raw: f64) -> Self {
        self.raw_value = Some(raw);
        self
    }

    /// Attaches the LMU time of the triggering sample.
    pub fn at(mut self, source_et_s: Option<f64>) -> Self {
        self.source_et_s = source_et_s;
        self
    }
}

/// Deduplicates notices per session context: each (code, channel) is emitted once until [`NoticeLog::reset`].
/// Bounded by the number of codes × registry channels.
#[derive(Debug, Default)]
pub struct NoticeLog {
    raised: BTreeSet<(NoticeCode, Option<ChannelId>)>,
}

impl NoticeLog {
    /// Pushes `notice` to `out` unless the same (code, channel) was already raised; logs it.
    pub fn raise(&mut self, notice: DiagnosticNotice, out: &mut Vec<DiagnosticNotice>) {
        if self.raised.insert((notice.code, notice.channel)) {
            push_logged(notice, out);
        }
    }

    /// Forgets which notices were raised (new session context).
    pub fn reset(&mut self) {
        self.raised.clear();
    }
}

/// Pushes `notice` to `out` and logs it (no deduplication; for rare one-off events).
pub fn push_logged(notice: DiagnosticNotice, out: &mut Vec<DiagnosticNotice>) {
    match notice.severity {
        Severity::Info | Severity::Notice => tracing::info!(
            component = "lmu-normalizer",
            code = ?notice.code,
            channel = ?notice.channel,
            raw = ?notice.raw_value,
            "{}",
            notice.message
        ),
        Severity::Warning | Severity::Critical => tracing::warn!(
            component = "lmu-normalizer",
            code = ?notice.code,
            channel = ?notice.channel,
            raw = ?notice.raw_value,
            "{}",
            notice.message
        ),
    }
    out.push(notice);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_are_raised_once_per_session() {
        let mut log = NoticeLog::default();
        let mut out = Vec::new();
        let n = || DiagnosticNotice::new(NoticeCode::EtRegression, Severity::Notice, None, "x");
        log.raise(n(), &mut out);
        log.raise(n(), &mut out);
        assert_eq!(out.len(), 1);
        log.reset();
        log.raise(n(), &mut out);
        assert_eq!(out.len(), 2);
    }
}

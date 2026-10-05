//! Runtime unit and scale detectors for LMU fields whose unit the header does not state or states wrongly.
//!
//! Every detector is pure, deterministic and session scoped: the normalizer resets them on a session,
//! track or vehicle change and on an ET regression. Decisions are latched (never flip back and forth) and
//! announced once through a [`crate::notice::DiagnosticNotice`]. None of them ever invents a value: when a
//! raw value cannot be interpreted it is published as `INVALID` or withheld, never as 0.

use serde::{Deserialize, Serialize};
use telemetry_model::units::{KELVIN_OFFSET, convert};

// ---------------------------------------------------------------------------------------------------------
// Brake temperature unit (A-LMU-015)
// ---------------------------------------------------------------------------------------------------------

/// Evidence window (s of LMU time) after the car is seen **entering** realtime (`mInRealtime` false → true,
/// i.e. the car was just put on track from the monitor/garage with its brakes at ambient). Cold-brake
/// evidence is only trusted in this window. A detector created while the car is already in realtime (app
/// started or state reset mid-session, possibly with hot brakes) never opens the window until the next
/// realtime entry.
pub const BRAKE_COLD_WINDOW_S: f64 = 10.0;
/// Maximum spread (max − min, raw units, same in K and °C) between the four wheels for a cold-brake vote:
/// brakes at ambient read alike, brakes cooling after a run do not (fronts and rears differ).
pub const BRAKE_COLD_MAX_SPREAD: f64 = 5.0;
/// Maximum speed (m/s) for a cold-brake evidence sample (car stationary or creeping out of the garage).
pub const BRAKE_COLD_MAX_SPEED_MPS: f64 = 5.0;
/// Kelvin hypothesis: all four raw cold readings within this band (K).
pub const BRAKE_COLD_KELVIN_BAND: (f64, f64) = (250.0, 350.0);
/// Celsius hypothesis: all four raw cold readings within this band (°C).
pub const BRAKE_COLD_CELSIUS_BAND: (f64, f64) = (-30.0, 80.0);
/// Maximum distance between the coldest brake and ambient (in the hypothesised unit) for a vote.
pub const BRAKE_AMBIENT_TOLERANCE: f64 = 25.0;
/// A raw reading below this cannot be Kelvin (it would be colder than −73 °C): Celsius evidence at any time.
/// Readings below [`BRAKE_RAW_FLOOR`] are garbage and give no evidence.
pub const BRAKE_KELVIN_IMPOSSIBLE_BELOW: f64 = 200.0;
/// Lowest raw reading treated as a temperature at all (the registry's −50 °C floor).
pub const BRAKE_RAW_FLOOR: f64 = -50.0;
/// A raw reading above this cannot be Celsius (above the 1500 °C plausibility bound): Kelvin evidence.
pub const BRAKE_CELSIUS_IMPOSSIBLE_ABOVE: f64 = 1_500.0;
/// Agreeing evidence samples needed to decide the unit.
pub const BRAKE_UNIT_VOTES_REQUIRED: u32 = 20;

/// Brake temperature unit hypothesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BrakeTempUnit {
    Kelvin,
    Celsius,
}

/// LMU `gameVersion`s whose `mBrakeTemp` unit was verified with real data (A-LMU-015). 14200 (LMU 1.4.2.0):
/// Gate 1 on 2026-10-02, cold brakes read exactly ambient + 273.15 on all four wheels (docs/GATE1_REPORT.md).
pub const VERIFIED_BRAKE_TEMP_UNITS: &[(i32, BrakeTempUnit)] = &[(14_200, BrakeTempUnit::Kelvin)];

/// The brake temperature unit verified for an LMU `gameVersion`, if any.
pub fn verified_brake_temp_unit(game_version: i32) -> Option<BrakeTempUnit> {
    VERIFIED_BRAKE_TEMP_UNITS
        .iter()
        .find(|(version, _)| *version == game_version)
        .map(|&(_, unit)| unit)
}

/// How firm the brake temperature unit is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnitDecisionState {
    /// No (or not enough) evidence yet: the default hypothesis is used.
    Assumed,
    /// Verified with real data for the running LMU version; kept unless session evidence contradicts it.
    Verified,
    /// Enough agreeing evidence: latched for the session.
    Decided,
    /// Evidence for both units (or against the verified unit): the values are withheld and flagged.
    Conflicting,
}

/// The brake temperature unit currently applied, with its confidence.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BrakeTempUnitDecision {
    /// Unit used to convert `mBrakeTemp` right now.
    pub unit: BrakeTempUnit,
    pub state: UnitDecisionState,
    /// 0..1: share of the required agreeing votes collected (1 once decided, 0 when conflicting).
    pub confidence: f32,
    pub kelvin_votes: u32,
    pub celsius_votes: u32,
}

/// What a call to [`BrakeTempUnitDetector::observe`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrakeUnitTransition {
    None,
    Decided(BrakeTempUnit),
    BecameConflicting,
}

/// Inputs of one brake-unit observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrakeObservation {
    /// Raw `mBrakeTemp` of the four wheels.
    pub raw: [f64; 4],
    /// Player telemetry ET (s).
    pub et_s: f64,
    /// Vehicle speed (m/s), if valid.
    pub speed_mps: Option<f64>,
    /// `scoringInfo.mInRealtime`.
    pub in_realtime: bool,
    /// `scoringInfo.mAmbientTemp` (°C, header unit VERIFIED), if valid.
    pub ambient_c: Option<f64>,
}

/// Decides per session whether `mBrakeTemp` is Kelvin or Celsius from evidence (A-LMU-015).
///
/// Policy (docs/ASSUMPTIONS.md A-LMU-015): the documented mitigation is "if undecided, publish SUSPECT";
/// the model has no SUSPECT quality, so until the unit is **decided** the brake temperatures are withheld
/// (`UNAVAILABLE`, raw value in the `BrakeTempUnitUnverified` notice, capability note "unit unverified").
/// An unverified conversion is never published as `VALID`: a wrong guess is a silent 273 °C error.
///
/// Evidence:
/// - **cold brakes** (within [`BRAKE_COLD_WINDOW_S`] of the car *entering* realtime, speed <
///   [`BRAKE_COLD_MAX_SPEED_MPS`], ambient known, the four wheels within [`BRAKE_COLD_MAX_SPREAD`] of each
///   other): all four readings in the Kelvin band and the coldest within ±[`BRAKE_AMBIENT_TOLERANCE`] of
///   ambient + 273.15 → Kelvin vote; all four in the Celsius band and the coldest within the tolerance of
///   ambient → Celsius vote;
/// - **impossibility** (any time): all four readings non-zero and in
///   [`BRAKE_RAW_FLOOR`]..[`BRAKE_KELVIN_IMPOSSIBLE_BELOW`] cannot be Kelvin → Celsius vote; a reading above
///   [`BRAKE_CELSIUS_IMPOSSIBLE_ABOVE`] (but still a plausible Kelvin value) cannot be Celsius → Kelvin vote.
///   All-zero readings (car not loaded) are never evidence.
///
/// [`BRAKE_UNIT_VOTES_REQUIRED`] votes for one unit with none for the other decide it.
///
/// For an LMU version listed in [`VERIFIED_BRAKE_TEMP_UNITS`] the verified unit is applied from the first
/// frame ([`UnitDecisionState::Verified`]); a single vote against it makes the detector conflicting (values
/// withheld again), because it would mean LMU changed the unit.
#[derive(Debug, Clone, Default)]
pub struct BrakeTempUnitDetector {
    kelvin_votes: u32,
    celsius_votes: u32,
    decided: Option<BrakeTempUnit>,
    conflicting: bool,
    /// Unit verified for the running LMU version (see [`BrakeTempUnitDetector::set_verified_unit`]).
    verified: Option<BrakeTempUnit>,
    /// `mInRealtime` of the previous snapshot (`None` before the first one).
    prev_in_realtime: Option<bool>,
    /// A realtime entry (false → true) was seen and the car has not left realtime since.
    entered_realtime: bool,
    /// Player ET of the first observation after the realtime entry (start of the cold window).
    realtime_since_et: Option<f64>,
}

impl BrakeTempUnitDetector {
    /// Leading hypothesis while undecided (reported in [`BrakeTempUnitDecision::unit`]; values are not
    /// published until the unit is decided).
    pub const DEFAULT_UNIT: BrakeTempUnit = BrakeTempUnit::Kelvin;

    /// Feeds `scoringInfo.mInRealtime` of every snapshot (with or without player telemetry), so that the
    /// realtime entry that opens the cold-brake window is seen even when no telemetry is published outside
    /// realtime. Idempotent per snapshot.
    pub fn note_realtime(&mut self, in_realtime: bool) {
        match (self.prev_in_realtime, in_realtime) {
            (Some(false), true) => {
                self.entered_realtime = true;
                self.realtime_since_et = None;
            }
            (_, false) => {
                self.entered_realtime = false;
                self.realtime_since_et = None;
            }
            _ => {}
        }
        self.prev_in_realtime = Some(in_realtime);
    }

    /// Sets the unit verified for the running LMU version (`None` for an unverified version). Called on
    /// every observation, so it follows a version change and survives detector resets.
    pub fn set_verified_unit(&mut self, unit: Option<BrakeTempUnit>) {
        self.verified = unit;
    }

    /// `true` when the unit is decided from evidence or verified for this LMU version and not contradicted
    /// (only then are values published).
    pub fn is_decided(&self) -> bool {
        self.decided.is_some() || (self.verified.is_some() && !self.conflicting)
    }

    /// The unit to apply now and the state of the decision.
    pub fn decision(&self) -> BrakeTempUnitDecision {
        let (unit, state, confidence) = match (self.decided, self.conflicting, self.verified) {
            (Some(unit), _, _) => (unit, UnitDecisionState::Decided, 1.0),
            (None, true, _) => (Self::DEFAULT_UNIT, UnitDecisionState::Conflicting, 0.0),
            (None, false, Some(unit)) => (unit, UnitDecisionState::Verified, 1.0),
            (None, false, None) => {
                let votes = self.kelvin_votes.max(self.celsius_votes);
                let share = f64::from(votes) / f64::from(BRAKE_UNIT_VOTES_REQUIRED);
                (
                    Self::DEFAULT_UNIT,
                    UnitDecisionState::Assumed,
                    share.min(1.0) as f32,
                )
            }
        };
        BrakeTempUnitDecision {
            unit,
            state,
            confidence,
            kelvin_votes: self.kelvin_votes,
            celsius_votes: self.celsius_votes,
        }
    }

    /// Converts a raw reading to °C with the unit currently applied.
    pub fn to_celsius(&self, raw: f64) -> f64 {
        match self.decision().unit {
            BrakeTempUnit::Kelvin => convert::kelvin_to_celsius(raw),
            BrakeTempUnit::Celsius => raw,
        }
    }

    /// Feeds one observation; returns what changed.
    pub fn observe(&mut self, obs: &BrakeObservation) -> BrakeUnitTransition {
        self.note_realtime(obs.in_realtime);
        if obs.in_realtime && self.entered_realtime && self.realtime_since_et.is_none() {
            self.realtime_since_et = Some(obs.et_s);
        }
        if self.decided.is_some() || self.conflicting {
            return BrakeUnitTransition::None;
        }
        if obs.raw.iter().any(|v| !v.is_finite()) {
            return BrakeUnitTransition::None;
        }
        let Some(vote) = Self::vote(obs, self.realtime_since_et) else {
            return BrakeUnitTransition::None;
        };
        // Evidence against the unit verified for this LMU version: LMU changed something; stop trusting it.
        if self.verified.is_some_and(|verified| verified != vote) {
            self.conflicting = true;
            return BrakeUnitTransition::BecameConflicting;
        }
        match vote {
            BrakeTempUnit::Kelvin => self.kelvin_votes = self.kelvin_votes.saturating_add(1),
            BrakeTempUnit::Celsius => self.celsius_votes = self.celsius_votes.saturating_add(1),
        }
        if self.kelvin_votes > 0 && self.celsius_votes > 0 {
            self.conflicting = true;
            return BrakeUnitTransition::BecameConflicting;
        }
        let winner = if self.kelvin_votes >= BRAKE_UNIT_VOTES_REQUIRED {
            Some(BrakeTempUnit::Kelvin)
        } else if self.celsius_votes >= BRAKE_UNIT_VOTES_REQUIRED {
            Some(BrakeTempUnit::Celsius)
        } else {
            None
        };
        match winner {
            Some(unit) => {
                self.decided = Some(unit);
                BrakeUnitTransition::Decided(unit)
            }
            None => BrakeUnitTransition::None,
        }
    }

    fn vote(obs: &BrakeObservation, realtime_since_et: Option<f64>) -> Option<BrakeTempUnit> {
        // Exactly 0 on every wheel is what a car that is not loaded yet reports: no evidence at all.
        if obs.raw.iter().all(|&v| v == 0.0) {
            return None;
        }
        // All four wheels (not one glitching wheel) must be impossible as Kelvin.
        let impossible_kelvin = obs
            .raw
            .iter()
            .all(|&v| v != 0.0 && (BRAKE_RAW_FLOOR..BRAKE_KELVIN_IMPOSSIBLE_BELOW).contains(&v));
        // One wheel suffices here, but only within the Kelvin plausibility bound (garbage gives nothing).
        let impossible_celsius = obs.raw.iter().any(|&v| {
            v > BRAKE_CELSIUS_IMPOSSIBLE_ABOVE
                && v <= BRAKE_CELSIUS_IMPOSSIBLE_ABOVE + KELVIN_OFFSET
        });
        match (impossible_kelvin, impossible_celsius) {
            (true, false) => return Some(BrakeTempUnit::Celsius),
            (false, true) => return Some(BrakeTempUnit::Kelvin),
            // Both at once: garbage across wheels, no evidence.
            (true, true) => return None,
            (false, false) => {}
        }
        let in_window = obs.in_realtime
            && realtime_since_et.is_some_and(|start| obs.et_s - start <= BRAKE_COLD_WINDOW_S);
        let slow = obs.speed_mps.is_some_and(|s| s < BRAKE_COLD_MAX_SPEED_MPS);
        let ambient = obs.ambient_c.filter(|a| a.is_finite())?;
        if !(in_window && slow) {
            return None;
        }
        let coldest = obs.raw.iter().copied().fold(f64::INFINITY, f64::min);
        let hottest = obs.raw.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if hottest - coldest > BRAKE_COLD_MAX_SPREAD {
            return None;
        }
        let all_in = |(lo, hi): (f64, f64)| obs.raw.iter().all(|&v| (lo..=hi).contains(&v));
        if all_in(BRAKE_COLD_KELVIN_BAND)
            && (coldest - (ambient + KELVIN_OFFSET)).abs() <= BRAKE_AMBIENT_TOLERANCE
        {
            Some(BrakeTempUnit::Kelvin)
        } else if all_in(BRAKE_COLD_CELSIUS_BAND)
            && (coldest - ambient).abs() <= BRAKE_AMBIENT_TOLERANCE
        {
            Some(BrakeTempUnit::Celsius)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Fraction scale (mVirtualEnergy, mSoC: A-LMU-013, A-LMU-014)
// ---------------------------------------------------------------------------------------------------------

/// Above this a fraction channel is taken to be in percent (LMU_CHANNEL_MAP §7).
pub const PERCENT_SCALE_THRESHOLD: f64 = 1.5;
/// Full scale of a percent value.
pub const PERCENT_FULL_SCALE: f64 = 100.0;
/// Highest raw value accepted as percent (1 % tolerance); anything above is an unknown scale.
pub const PERCENT_MAX_TOLERANT: f64 = 101.0;
/// Lowest raw value accepted in either scale (1 % tolerance below 0).
pub const FRACTION_MIN_TOLERANT: f64 = -0.01;
/// Completed laps (telemetry `mLapNumber` advance) over which every value must stay in (0, 1.5] before the
/// 0..1 scale is accepted. In percent such values mean at most 1.5 % left: VE or a battery that low cannot
/// last two laps while still changing (a lap uses several %), so this is positive evidence for 0..1.
pub const FRACTION_CONFIRM_LAPS: i32 = 2;
/// Minimum change (max - min, raw) during the confirmation laps: the value must actually be in use.
pub const FRACTION_CONFIRM_MIN_SPAN: f64 = 0.02;

/// Scale of a fraction-like LMU field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FractionScale {
    /// No decisive evidence yet (every value so far is in 0..1.5, which fits both scales): withheld
    /// (`UNAVAILABLE`), never guessed.
    Undecided,
    /// 0..1, confirmed by [`FRACTION_CONFIRM_LAPS`] laps of changing values in (0, 1.5].
    Fraction,
    /// 0..100: divided by 100 (latched after a value above [`PERCENT_SCALE_THRESHOLD`]).
    Percent,
    /// A value outside every known scale (or a value above 1.5 after 0..1 was confirmed) was seen: the
    /// channel is INVALID for the session. Never converted to an invented unit (e.g. MJ).
    Unrecognized,
}

/// Running confirmation of the 0..1 scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FractionConfirmation {
    start_lap: i32,
    min: f64,
    max: f64,
}

/// Per-session scale detector for one fraction-like field (A-LMU-013/014).
#[derive(Debug, Clone)]
pub struct FractionScaleDetector {
    scale: FractionScale,
    confirmation: Option<FractionConfirmation>,
}

impl Default for FractionScaleDetector {
    fn default() -> Self {
        Self {
            scale: FractionScale::Undecided,
            confirmation: None,
        }
    }
}

impl FractionScaleDetector {
    /// Current scale.
    pub fn scale(&self) -> FractionScale {
        self.scale
    }

    /// Feeds a raw value and the player's current lap number; returns the new scale if it changed (latched
    /// transitions only).
    pub fn observe(&mut self, raw: f64, lap: i32) -> Option<FractionScale> {
        if !raw.is_finite() || self.scale == FractionScale::Unrecognized {
            return None;
        }
        let next = if !(FRACTION_MIN_TOLERANT..=PERCENT_MAX_TOLERANT).contains(&raw) {
            FractionScale::Unrecognized
        } else if raw > PERCENT_SCALE_THRESHOLD {
            match self.scale {
                // A 0..1 field confirmed over laps never reads above 1.5: conflicting evidence.
                FractionScale::Fraction => FractionScale::Unrecognized,
                _ => FractionScale::Percent,
            }
        } else if self.scale == FractionScale::Undecided {
            self.confirm_fraction(raw, lap)
        } else {
            self.scale
        };
        (next != self.scale).then(|| {
            self.scale = next;
            next
        })
    }

    /// Tracks the 0..1 confirmation with a value in the ambiguous range; returns the resulting scale.
    fn confirm_fraction(&mut self, raw: f64, lap: i32) -> FractionScale {
        // Zero (empty / not in use) or a lap counter that went back restarts the confirmation.
        if raw <= 0.0 {
            self.confirmation = None;
            return FractionScale::Undecided;
        }
        if self.confirmation.is_some_and(|c| lap < c.start_lap) {
            self.confirmation = None;
        }
        let c = self.confirmation.get_or_insert(FractionConfirmation {
            start_lap: lap,
            min: raw,
            max: raw,
        });
        c.min = c.min.min(raw);
        c.max = c.max.max(raw);
        let laps = lap.saturating_sub(c.start_lap);
        if laps >= FRACTION_CONFIRM_LAPS && c.max - c.min >= FRACTION_CONFIRM_MIN_SPAN {
            FractionScale::Fraction
        } else {
            FractionScale::Undecided
        }
    }

    /// Converts `raw` to a 0..1 fraction under the current scale; `None` while undecided or unrecognized.
    pub fn to_fraction(&self, raw: f64) -> Option<f64> {
        match self.scale {
            FractionScale::Fraction => Some(raw),
            FractionScale::Percent => Some(raw / PERCENT_FULL_SCALE),
            FractionScale::Undecided | FractionScale::Unrecognized => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Brake pressure (A-LMU-017)
// ---------------------------------------------------------------------------------------------------------

/// A `mBrakePressure` above this means it is no longer the documented 0..1 fraction (LMU_CHANNEL_MAP §12).
pub const BRAKE_PRESSURE_UNIT_CHANGE_THRESHOLD: f64 = 1.5;

/// Watches `mBrakePressure` for the announced switch to kPa. Once tripped, mapping stops for the session:
/// the channel is INVALID rather than a fraction that is really a pressure.
#[derive(Debug, Clone, Default)]
pub struct BrakePressureGuard {
    unit_changed: bool,
}

impl BrakePressureGuard {
    /// Feeds the four raw values; returns `Some(trigger)` the first time the unit change is detected.
    pub fn observe(&mut self, raw: [f64; 4]) -> Option<f64> {
        if self.unit_changed {
            return None;
        }
        let trigger = raw
            .iter()
            .copied()
            .find(|v| v.is_finite() && *v > BRAKE_PRESSURE_UNIT_CHANGE_THRESHOLD)?;
        self.unit_changed = true;
        Some(trigger)
    }

    /// `true` once a unit change was detected.
    pub fn unit_changed(&self) -> bool {
        self.unit_changed
    }
}

// ---------------------------------------------------------------------------------------------------------
// Temperatures without a header unit (A-LMU-038)
// ---------------------------------------------------------------------------------------------------------

/// A reading above this is taken as Kelvin (LMU_CHANNEL_MAP §7, §11).
pub const KELVIN_LATCH_THRESHOLD: f64 = 200.0;

/// Latches "this field is Kelvin" after a reading above [`KELVIN_LATCH_THRESHOLD`]; °C (the CLAIMED unit)
/// until then.
#[derive(Debug, Clone, Default)]
pub struct KelvinLatch {
    kelvin: bool,
}

impl KelvinLatch {
    /// Feeds a raw value; returns `true` the first time Kelvin is latched.
    pub fn observe(&mut self, raw: f64) -> bool {
        if !self.kelvin && raw.is_finite() && raw > KELVIN_LATCH_THRESHOLD {
            self.kelvin = true;
            return true;
        }
        false
    }

    /// Converts to °C under the current latch.
    pub fn to_celsius(&self, raw: f64) -> f64 {
        if self.kelvin {
            convert::kelvin_to_celsius(raw)
        } else {
            raw
        }
    }

    /// `true` once Kelvin is latched.
    pub fn is_kelvin(&self) -> bool {
        self.kelvin
    }
}

// ---------------------------------------------------------------------------------------------------------
// Turbo boost (A-LMU-027)
// ---------------------------------------------------------------------------------------------------------

/// Raw band read as kPa (absolute boost pressure 0.5..5 bar).
pub const TURBO_KPA_BAND: (f64, f64) = (50.0, 500.0);

/// Unit of `mTurboBoostPressure`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TurboUnit {
    Pascal,
    Kilopascal,
}

/// Latches the turbo boost unit on the first non-zero reading: in [`TURBO_KPA_BAND`] → kPa, otherwise Pa
/// (DuckDB shows ≈ 101,300 Pa on a GT3). Exactly 0 means the car reports no turbo.
#[derive(Debug, Clone, Default)]
pub struct TurboUnitDetector {
    unit: Option<TurboUnit>,
}

/// Interpretation of one turbo reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TurboReading {
    /// The car reports no turbo (raw exactly 0).
    NotReported,
    /// Pressure in Pa.
    Pascal(f64),
    /// Not finite.
    Invalid,
}

impl TurboUnitDetector {
    /// Interprets `raw`; the second value is `Some(unit)` when the unit was latched by this call.
    pub fn read(&mut self, raw: f64) -> (TurboReading, Option<TurboUnit>) {
        if !raw.is_finite() {
            return (TurboReading::Invalid, None);
        }
        if raw == 0.0 {
            return (TurboReading::NotReported, None);
        }
        let mut latched = None;
        let unit = *self.unit.get_or_insert_with(|| {
            let (lo, hi) = TURBO_KPA_BAND;
            let unit = if (lo..=hi).contains(&raw) {
                TurboUnit::Kilopascal
            } else {
                TurboUnit::Pascal
            };
            latched = Some(unit);
            unit
        });
        let pa = match unit {
            TurboUnit::Pascal => raw,
            TurboUnit::Kilopascal => convert::kpa_to_pa(raw),
        };
        (TurboReading::Pascal(pa), latched)
    }
}

// ---------------------------------------------------------------------------------------------------------
// Time of day (A-LMU-036)
// ---------------------------------------------------------------------------------------------------------

/// Largest value that could still be hours.
pub const TIME_OF_DAY_MAX_HOURS: f64 = 24.0;
/// Seconds per day (upper bound of seconds since midnight).
pub const SECONDS_PER_DAY: f64 = 86_400.0;

/// `mTimeOfDay` scale: seconds since midnight once any value above 24 has been seen; until then hours vs
/// seconds is undecided and the value is withheld (never guessed).
#[derive(Debug, Clone, Default)]
pub struct TimeOfDayDetector {
    seconds: bool,
}

impl TimeOfDayDetector {
    /// Seconds since midnight, or `None` while undecided or out of range.
    pub fn read(&mut self, raw: f64) -> Option<f64> {
        if !raw.is_finite() {
            return None;
        }
        if raw > TIME_OF_DAY_MAX_HOURS {
            self.seconds = true;
        }
        (self.seconds && (0.0..=SECONDS_PER_DAY).contains(&raw)).then_some(raw)
    }

    /// `true` once the scale is known to be seconds.
    pub fn is_decided(&self) -> bool {
        self.seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(raw: [f64; 4], et_s: f64, speed: f64, ambient: f64) -> BrakeObservation {
        BrakeObservation {
            raw,
            et_s,
            speed_mps: Some(speed),
            in_realtime: true,
            ambient_c: Some(ambient),
        }
    }

    #[test]
    fn cold_brakes_near_ambient_plus_273_decide_kelvin() {
        let mut d = BrakeTempUnitDetector::default();
        d.note_realtime(false); // monitor / garage screen, then the car is put on track
        assert_eq!(d.decision().state, UnitDecisionState::Assumed);
        assert_eq!(d.decision().unit, BrakeTempUnit::Kelvin);
        let mut last = BrakeUnitTransition::None;
        for i in 0..BRAKE_UNIT_VOTES_REQUIRED {
            let et = 100.0 + f64::from(i) * 0.01;
            last = d.observe(&obs([296.0, 297.0, 295.5, 296.2], et, 0.0, 22.0));
        }
        assert_eq!(last, BrakeUnitTransition::Decided(BrakeTempUnit::Kelvin));
        let dec = d.decision();
        assert_eq!(
            (dec.state, dec.unit, dec.confidence),
            (UnitDecisionState::Decided, BrakeTempUnit::Kelvin, 1.0)
        );
        assert!((d.to_celsius(373.15) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn cold_brakes_near_ambient_decide_celsius() {
        let mut d = BrakeTempUnitDetector::default();
        for i in 0..BRAKE_UNIT_VOTES_REQUIRED {
            d.observe(&obs(
                [23.0, 22.5, 24.0, 23.1],
                5.0 + f64::from(i) * 0.01,
                1.0,
                22.0,
            ));
        }
        assert_eq!(d.decision().unit, BrakeTempUnit::Celsius);
        assert!((d.to_celsius(400.0) - 400.0).abs() < 1e-9);
    }

    #[test]
    fn hot_or_moving_or_late_samples_give_no_cold_evidence() {
        let mut d = BrakeTempUnitDetector::default();
        d.note_realtime(false);
        d.observe(&obs([296.0; 4], 0.0, 0.0, 22.0)); // starts the window, one vote
        assert_eq!(d.decision().kelvin_votes, 1);
        d.observe(&obs([296.0; 4], 1.0, 30.0, 22.0)); // moving
        d.observe(&obs([296.0; 4], 20.0, 0.0, 22.0)); // after the window
        d.observe(&obs([700.0; 4], 2.0, 0.0, 22.0)); // hot
        let partial = d.decision();
        assert_eq!(partial.kelvin_votes, 1);
        assert_eq!(partial.state, UnitDecisionState::Assumed);
        assert!(partial.confidence > 0.0 && partial.confidence < 1.0);
    }

    #[test]
    fn cold_window_needs_a_realtime_entry_and_alike_wheels() {
        // Detector created mid-session while already in realtime (app started during a pit stop): hot brakes
        // that happen to read 280..310 (really °C) never vote Kelvin.
        let mut late = BrakeTempUnitDetector::default();
        for i in 0..100 {
            late.observe(&obs([296.0; 4], 1_000.0 + f64::from(i) * 0.01, 0.0, 22.0));
        }
        assert_eq!(late.decision().kelvin_votes, 0, "no realtime entry seen");
        assert!(!late.is_decided());
        // Leaving and re-entering realtime (car put back on track) opens a new window.
        late.note_realtime(false);
        late.observe(&obs([296.0; 4], 1_100.0, 0.0, 22.0));
        assert_eq!(late.decision().kelvin_votes, 1);

        // Wheels far apart (brakes cooling after a run) are not cold-brake evidence.
        let mut spread = BrakeTempUnitDetector::default();
        spread.note_realtime(false);
        spread.observe(&obs([285.0, 300.0, 290.0, 310.0], 0.0, 0.0, 22.0));
        assert_eq!(spread.decision().kelvin_votes, 0);
    }

    #[test]
    fn impossible_readings_are_evidence_at_any_time() {
        let mut d = BrakeTempUnitDetector::default();
        // 150 cannot be Kelvin (−123 °C): Celsius, even while driving fast late in the session.
        d.observe(&obs([150.0, 160.0, 140.0, 150.0], 3_000.0, 60.0, 22.0));
        assert_eq!(d.decision().celsius_votes, 1);
        // Conflicting evidence: keep the default and flag it.
        let t = d.observe(&obs([1_600.0, 900.0, 900.0, 900.0], 3_001.0, 60.0, 22.0));
        assert_eq!(t, BrakeUnitTransition::BecameConflicting);
        let dec = d.decision();
        assert_eq!(
            (dec.state, dec.unit, dec.confidence),
            (UnitDecisionState::Conflicting, BrakeTempUnit::Kelvin, 0.0)
        );
    }

    #[test]
    fn non_finite_brake_values_are_ignored() {
        let mut d = BrakeTempUnitDetector::default();
        d.observe(&obs([f64::NAN, 296.0, 296.0, 296.0], 0.0, 0.0, 22.0));
        assert_eq!(d.decision().kelvin_votes, 0);
        // A car that is not loaded reports zeros: never evidence for Celsius.
        d.observe(&obs([0.0; 4], 0.1, 0.0, 22.0));
        // One glitching wheel is not enough to rule Kelvin out.
        d.observe(&obs([20.0, 296.0, 296.0, 296.0], 50.0, 40.0, 22.0));
        // Absurd garbage is not Kelvin evidence either.
        d.observe(&obs([1.0e9, 296.0, 296.0, 296.0], 50.1, 40.0, 22.0));
        assert_eq!(
            (d.decision().kelvin_votes, d.decision().celsius_votes),
            (0, 0)
        );
    }

    #[test]
    fn fraction_scale_detection() {
        let mut d = FractionScaleDetector::default();
        assert_eq!(d.scale(), FractionScale::Undecided);
        assert_eq!(d.observe(0.8, 1), None);
        assert_eq!(
            d.to_fraction(0.8),
            None,
            "ambiguous: withheld, never guessed"
        );
        assert_eq!(d.observe(87.0, 1), Some(FractionScale::Percent));
        assert_eq!(d.to_fraction(50.0), Some(0.5));
        assert_eq!(d.observe(1.0, 1), None, "latched");
        assert_eq!(d.observe(5_000.0, 1), Some(FractionScale::Unrecognized));
        assert_eq!(d.to_fraction(0.5), None);
        assert_eq!(d.observe(0.5, 1), None, "unrecognized is final");
        let mut neg = FractionScaleDetector::default();
        assert_eq!(neg.observe(-3.0, 1), Some(FractionScale::Unrecognized));
        assert_eq!(neg.observe(f64::NAN, 1), None);
    }

    #[test]
    fn low_percent_value_at_a_late_start_is_not_taken_as_a_fraction() {
        // Started late in a stint with 0.9 % VE left: never published as 90 %.
        let mut d = FractionScaleDetector::default();
        for _ in 0..1_000 {
            assert_eq!(d.observe(0.9, 12), None);
        }
        assert_eq!(d.scale(), FractionScale::Undecided);
        assert_eq!(d.to_fraction(0.9), None);
        // Running out (0) restarts the confirmation; the refill to 100 decides percent.
        d.observe(0.4, 13);
        d.observe(0.0, 14);
        assert_eq!(d.observe(100.0, 15), Some(FractionScale::Percent));
    }

    #[test]
    fn fraction_is_confirmed_after_laps_of_changing_values() {
        let mut d = FractionScaleDetector::default();
        assert_eq!(d.observe(0.95, 3), None);
        assert_eq!(d.observe(0.90, 4), None, "one lap is not enough");
        assert_eq!(d.observe(0.85, 5), Some(FractionScale::Fraction));
        assert_eq!(d.to_fraction(0.85), Some(0.85));
        // A value above 1.5 after 0..1 was confirmed is conflicting evidence: INVALID, not percent.
        assert_eq!(d.observe(60.0, 6), Some(FractionScale::Unrecognized));

        // A constant value never confirms (not in use).
        let mut flat = FractionScaleDetector::default();
        for lap in 0..10 {
            flat.observe(0.5, lap);
        }
        assert_eq!(flat.scale(), FractionScale::Undecided);
        // A lap counter that goes back (restart) restarts the confirmation.
        let mut back = FractionScaleDetector::default();
        back.observe(0.9, 10);
        back.observe(0.8, 2);
        assert_eq!(back.observe(0.7, 3), None);
        assert_eq!(back.observe(0.6, 4), Some(FractionScale::Fraction));
    }

    #[test]
    fn brake_pressure_unit_change() {
        let mut g = BrakePressureGuard::default();
        assert_eq!(g.observe([0.4, 0.4, 0.6, 0.6]), None);
        assert_eq!(
            g.observe([1.01, 1.0, 0.0, 0.0]),
            None,
            "tolerance, not a unit change"
        );
        assert_eq!(g.observe([0.4, 2_500.0, 0.6, 0.6]), Some(2_500.0));
        assert!(g.unit_changed());
        assert_eq!(g.observe([0.4, 2_500.0, 0.6, 0.6]), None, "raised once");
    }

    #[test]
    fn kelvin_latch() {
        let mut l = KelvinLatch::default();
        assert!(!l.observe(85.0));
        assert!((l.to_celsius(85.0) - 85.0).abs() < 1e-12);
        assert!(l.observe(358.15));
        assert!(!l.observe(358.15));
        assert!((l.to_celsius(358.15) - 85.0).abs() < 1e-9);
    }

    #[test]
    fn turbo_unit() {
        let mut t = TurboUnitDetector::default();
        assert_eq!(t.read(0.0), (TurboReading::NotReported, None));
        assert_eq!(t.read(f64::NAN).0, TurboReading::Invalid);
        assert_eq!(
            t.read(101_300.0),
            (TurboReading::Pascal(101_300.0), Some(TurboUnit::Pascal))
        );
        let mut k = TurboUnitDetector::default();
        assert_eq!(
            k.read(180.0),
            (TurboReading::Pascal(180_000.0), Some(TurboUnit::Kilopascal))
        );
        assert_eq!(k.read(200.0), (TurboReading::Pascal(200_000.0), None));
    }

    #[test]
    fn time_of_day_is_withheld_until_seconds_are_proven() {
        let mut t = TimeOfDayDetector::default();
        assert_eq!(t.read(14.5), None, "hours or seconds? undecided");
        assert_eq!(t.read(52_200.0), Some(52_200.0));
        assert!(t.is_decided());
        assert_eq!(t.read(14.5), Some(14.5), "seconds latched");
        assert_eq!(t.read(90_000.0), None, "out of range");
    }
}

//! Per-vehicle capabilities decided from observed data (never from the car's name or class string), and
//! maintenance of the per-channel [`CapabilityMap`].

use telemetry_model::Quality;
use telemetry_model::channel::{FrequencyClass, Source};
use telemetry_model::enums::HybridMotorState;
use telemetry_model::frame::TelemetryFrame;
use telemetry_model::registry;
use telemetry_model::scoring::{CapabilityMap, CapabilityStatus, VehicleCapabilities};
use telemetry_model::{ChannelId, registry::ch};

use lmu_reader::RawTelemetry;

use crate::detect::PERCENT_MAX_TOLERANT;

/// Telemetry samples (≈ 3 s at 100 Hz) without any sign of an optional system before deciding the car does
/// not have it. A later sign upgrades the decision to `true` (never the other way round).
pub const CAPABILITY_DECISION_SAMPLES: u32 = 300;

/// Decides [`VehicleCapabilities`] for the current vehicle from its telemetry.
///
/// - Settings systems (TC, ABS, brake migration, motor map, anti-roll bars) are decided at once from their
///   `…Max` fields: a maximum of 0 means the car has no such adjustable system.
/// - Hybrid: `mElectricBoostMotorState != 0` (0 = unavailable) or a plausible non-zero `mSoC`.
/// - Virtual energy: any non-zero `mVirtualEnergy`.
/// - Tyre wear: any non-zero `mWear`.
/// - Rear flap: `mRearFlapActivated` or `mRearFlapLegalStatus` non-zero.
///
/// Hybrid, virtual energy and rear flap are `None` until a sign is seen (→ `Some(true)`) or
/// [`CAPABILITY_DECISION_SAMPLES`] samples pass without one (→ `Some(false)`). Tyre wear is never decided
/// absent: a zero `mWear` is ambiguous (wear not simulated, or a new tyre if 0 = unworn, direction
/// UNVERIFIED A-LMU-040), so it stays `None` until a non-zero value is seen.
///
/// Hybrid is never decided absent from samples taken in the garage stall (A-LMU-057; verify-v5 Hypercar:
/// the hybrid channels of a hybrid car were reported NOT_SUPPORTED in 1.8 % of frames, in the garage): only
/// samples outside the garage stall count towards its window ([`VehicleCapabilityTracker::observe_at`]).
#[derive(Debug, Clone, Default)]
pub struct VehicleCapabilityTracker {
    caps: VehicleCapabilities,
    samples: u32,
    /// Samples taken outside the garage stall (the hybrid decision window).
    hybrid_samples: u32,
}

impl VehicleCapabilityTracker {
    /// Current capabilities.
    pub fn capabilities(&self) -> VehicleCapabilities {
        self.caps
    }

    /// Forgets everything (vehicle or session change).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feeds one new player telemetry sample taken outside the garage stall (or where that is unknown);
    /// returns `true` if the capabilities changed.
    pub fn observe(&mut self, t: &RawTelemetry) -> bool {
        self.observe_at(t, false)
    }

    /// Feeds one new player telemetry sample; `in_garage_stall` is the player's scoring `mInGarageStall`.
    /// A sample in the garage stall can decide hybrid present but never counts towards deciding it absent.
    /// Returns `true` if the capabilities changed.
    pub fn observe_at(&mut self, t: &RawTelemetry, in_garage_stall: bool) -> bool {
        let before = self.caps;
        self.samples = self.samples.saturating_add(1);
        if !in_garage_stall {
            self.hybrid_samples = self.hybrid_samples.saturating_add(1);
        }
        let undecided_is_false = self.samples >= CAPABILITY_DECISION_SAMPLES;
        let hybrid_undecided_is_false = self.hybrid_samples >= CAPABILITY_DECISION_SAMPLES;

        let hybrid_state = HybridMotorState::from_raw(t.electric_boost_motor_state);
        let soc_plausible =
            t.soc.is_finite() && t.soc > 0.0 && f64::from(t.soc) <= PERCENT_MAX_TOLERANT;
        let hybrid_seen = hybrid_state != HybridMotorState::Unavailable || soc_plausible;
        observed(
            &mut self.caps.supports_hybrid,
            hybrid_seen,
            hybrid_undecided_is_false,
        );

        let ve_seen = t.virtual_energy.is_finite() && t.virtual_energy != 0.0;
        observed(
            &mut self.caps.supports_virtual_energy,
            ve_seen,
            undecided_is_false,
        );

        let wear_seen = t.wheels.iter().any(|w| w.wear.is_finite() && w.wear != 0.0);
        // Never decided `false` from zeros (fresh tyres in the garage read the same as "not simulated").
        observed(&mut self.caps.supports_tyre_wear, wear_seen, false);

        let flap_seen = t.rear_flap_activated != 0 || t.rear_flap_legal_status != 0;
        observed(
            &mut self.caps.supports_rear_flap,
            flap_seen,
            undecided_is_false,
        );

        self.caps.supports_tc = Some(t.tc_max > 0);
        self.caps.supports_abs = Some(t.abs_max > 0);
        self.caps.supports_brake_migration = Some(t.migration_max > 0);
        self.caps.supports_motor_map = Some(t.motor_map_max > 0);
        self.caps.supports_arb_adjustment =
            Some(t.front_anti_sway_max > 0 || t.rear_anti_sway_max > 0);
        // LMU exposes intervention only as booleans; the signal exists when the system exists or the flag
        // was ever seen set.
        let tc_signal = t.tc_max > 0 || t.tc_active;
        let abs_signal = t.abs_max > 0 || t.abs_active;
        observed(&mut self.caps.supports_tc_intervention, tc_signal, true);
        observed(&mut self.caps.supports_abs_intervention, abs_signal, true);

        self.caps != before
    }
}

/// `Some(true)` once seen (sticky); `Some(false)` when not seen and `decide_false`; otherwise unchanged.
fn observed(slot: &mut Option<bool>, seen: bool, decide_false: bool) {
    if seen {
        *slot = Some(true);
    } else if decide_false && slot.is_none() {
        *slot = Some(false);
    }
}

/// Capability status implied by a published sample of channel `id`.
fn status_of(id: ChannelId, quality: Quality) -> CapabilityStatus {
    let calculated = registry::by_id(id).is_some_and(|d| d.source == Source::Calculated);
    match quality {
        Quality::Valid if calculated => CapabilityStatus::Calculated,
        Quality::Valid => CapabilityStatus::Available,
        Quality::Invalid => CapabilityStatus::Invalid,
        Quality::Stale => CapabilityStatus::Stale,
        Quality::Unavailable => CapabilityStatus::Unavailable,
        Quality::NotSupported => CapabilityStatus::NotSupported,
    }
}

/// Updates statuses in `map` from the qualities of `frame`; returns `true` if any status changed. Only
/// LMU-sourced and calculated channels are touched (application diagnostics belong to other components).
pub fn update_from_frame(map: &mut CapabilityMap, frame: &TelemetryFrame) -> bool {
    let mut changed = false;
    for (def, sample) in frame.iter() {
        if !(def.source.is_lmu_shared_memory() || def.source == Source::Calculated) {
            continue;
        }
        let status = status_of(def.id, sample.quality);
        if let Some(cap) = map.channels.get_mut(&def.id)
            && cap.status != status
        {
            cap.status = status;
            changed = true;
        }
    }
    changed
}

/// Sets the note of `id` if it differs; returns `true` if it changed.
pub fn set_note(map: &mut CapabilityMap, id: ChannelId, note: Option<&str>) -> bool {
    match map.channels.get_mut(&id) {
        Some(cap) if cap.note.as_deref() != note => {
            cap.note = note.map(str::to_owned);
            true
        }
        _ => false,
    }
}

/// Records measured native rates: telemetry-block, generic-block and physics-rate calculated channels get
/// `telemetry_hz`; scoring-block and other calculated channels get `scoring_hz`. Rates are measured by the
/// reader (`ReaderStats::telemetry_rate` / `scoring_rate`), never assumed.
pub fn apply_native_rates(
    map: &mut CapabilityMap,
    telemetry_hz: Option<f64>,
    scoring_hz: Option<f64>,
) {
    for def in registry::all() {
        let rate = match def.source {
            Source::LmuSharedMemoryTelemetry | Source::LmuSharedMemoryGeneric => telemetry_hz,
            Source::LmuSharedMemoryScoring => scoring_hz,
            Source::Calculated if def.frequency == FrequencyClass::Physics => telemetry_hz,
            Source::Calculated => scoring_hz,
            Source::LmuRest | Source::Application | Source::LmuDuckDb => None,
        };
        if let Some(hz) = rate {
            // Narrowing to the f32 storage type of the capability map (rates are ≤ a few kHz).
            map.set_rate(def.id, hz as f32);
        }
    }
}

/// Channels whose value only exists while the hybrid motor reports a state other than "unavailable".
pub const HYBRID_GATED: [ChannelId; 7] = [
    ch::HYBRID_MOTOR_TORQUE,
    ch::HYBRID_MOTOR_RPM,
    ch::HYBRID_MOTOR_TEMP,
    ch::HYBRID_WATER_TEMP,
    ch::REGEN_POWER,
    ch::BATTERY_SOC,
    ch::BATTERY_CHARGE,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::zero_telemetry;

    #[test]
    fn non_hybrid_car_is_decided_after_the_window() {
        let mut tr = VehicleCapabilityTracker::default();
        let t = zero_telemetry();
        assert!(tr.observe(&t), "settings systems decided at once");
        assert_eq!(tr.capabilities().supports_hybrid, None);
        assert_eq!(tr.capabilities().supports_tc, Some(false));
        for _ in 1..CAPABILITY_DECISION_SAMPLES {
            tr.observe(&t);
        }
        let caps = tr.capabilities();
        assert_eq!(caps.supports_hybrid, Some(false));
        assert_eq!(caps.supports_virtual_energy, Some(false));
        assert_eq!(caps.supports_tc_intervention, Some(false));
    }

    #[test]
    fn hybrid_car_is_detected_from_data() {
        let mut tr = VehicleCapabilityTracker::default();
        let mut t = zero_telemetry();
        t.electric_boost_motor_state = 2; // propulsion
        t.virtual_energy = 0.93;
        t.tc_max = 11;
        assert!(tr.observe(&t));
        let caps = tr.capabilities();
        assert_eq!(caps.supports_hybrid, Some(true));
        assert_eq!(caps.supports_virtual_energy, Some(true));
        assert_eq!(caps.supports_tc, Some(true));
        assert_eq!(caps.supports_tc_intervention, Some(true));
        // Once seen, a later "unavailable" state does not downgrade within the vehicle session.
        t.electric_boost_motor_state = 0;
        for _ in 0..CAPABILITY_DECISION_SAMPLES {
            tr.observe(&t);
        }
        assert_eq!(tr.capabilities().supports_hybrid, Some(true));
    }

    /// Second normalizer review F4: a hybrid car in the garage stall reads motor state 0 and SoC 0; it must
    /// not be decided "no hybrid" there.
    #[test]
    fn hybrid_is_not_decided_absent_in_the_garage_stall() {
        let mut tr = VehicleCapabilityTracker::default();
        let mut t = zero_telemetry();
        for _ in 0..(3 * CAPABILITY_DECISION_SAMPLES) {
            tr.observe_at(&t, true);
        }
        let caps = tr.capabilities();
        assert_eq!(caps.supports_hybrid, None, "deferred in the garage");
        assert_eq!(
            caps.supports_virtual_energy,
            Some(false),
            "others unchanged"
        );
        // Leaving the garage with the motor reporting: present.
        t.electric_boost_motor_state = 2;
        assert!(tr.observe_at(&t, false));
        assert_eq!(tr.capabilities().supports_hybrid, Some(true));
        // A car that shows no hybrid outside the garage is still decided absent after the window.
        let mut gt = VehicleCapabilityTracker::default();
        let z = zero_telemetry();
        for _ in 0..CAPABILITY_DECISION_SAMPLES {
            gt.observe_at(&z, true);
        }
        for _ in 0..CAPABILITY_DECISION_SAMPLES {
            gt.observe_at(&z, false);
        }
        assert_eq!(gt.capabilities().supports_hybrid, Some(false));
    }

    #[test]
    fn plausible_soc_alone_means_hybrid() {
        let mut tr = VehicleCapabilityTracker::default();
        let mut t = zero_telemetry();
        t.soc = 55.0;
        tr.observe(&t);
        assert_eq!(tr.capabilities().supports_hybrid, Some(true));
    }

    #[test]
    fn late_evidence_upgrades_a_false_decision() {
        let mut tr = VehicleCapabilityTracker::default();
        let mut t = zero_telemetry();
        for _ in 0..CAPABILITY_DECISION_SAMPLES {
            tr.observe(&t);
        }
        assert_eq!(tr.capabilities().supports_rear_flap, Some(false));
        t.rear_flap_activated = 1;
        assert!(tr.observe(&t));
        assert_eq!(tr.capabilities().supports_rear_flap, Some(true));
    }

    #[test]
    fn zero_tyre_wear_is_never_decided_absent() {
        // A car on new tyres in the garage reads 0 for ever: undecided, not "no tyre wear".
        let mut tr = VehicleCapabilityTracker::default();
        let mut t = zero_telemetry();
        for _ in 0..(3 * CAPABILITY_DECISION_SAMPLES) {
            tr.observe(&t);
        }
        assert_eq!(tr.capabilities().supports_tyre_wear, None);
        t.wheels[2].wear = 0.01;
        assert!(tr.observe(&t));
        assert_eq!(tr.capabilities().supports_tyre_wear, Some(true));
    }

    #[test]
    fn rates_follow_the_source_block() {
        let mut map = CapabilityMap::from_registry();
        apply_native_rates(&mut map, Some(99.5), Some(5.0));
        let rate = |id| map.get(id).and_then(|c| c.native_rate_hz);
        assert_eq!(rate(ch::RPM), Some(99.5));
        assert_eq!(rate(ch::SPEED), Some(99.5));
        assert_eq!(rate(ch::POSITION), Some(5.0));
        assert_eq!(rate(ch::NET_RTT), None);
    }
}

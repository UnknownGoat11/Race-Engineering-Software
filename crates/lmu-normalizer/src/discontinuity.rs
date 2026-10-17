//! Known discontinuities of the car state (A-LMU-067).
//!
//! LMU moves the car between the garage stall and the pit lane in one frame (a "teleport": 9 m when the car
//! is released from the garage, 652 m on a return to the garage in the Gate-1 capture) and resets or services
//! it there: fuel, tyres, brake temperatures, energy and positions step. The registry rate guard would reject
//! the first sample of every such step. This module recognises the discontinuity from corroborating evidence
//! so that the normalizer resets the rate history of the affected channels instead (range checks still
//! apply). Anything that is not corroborated stays an implausible jump and is rejected.
//!
//! Pure and deterministic; the normalizer owns one [`DiscontinuityDetector`] per session context.

use serde::{Deserialize, Serialize};
use telemetry_model::registry::Wheel;
use telemetry_model::{ChannelId, ch};

use crate::mapping::wheel_id;

/// Smallest position step (m) between two consecutive samples that can be a teleport. The smallest teleport
/// seen at Gate 1 (LMU 1.4.2.0) is the garage release: 9.2 m in 12.5 ms. Steps shorter than this are never
/// treated as teleports (the rate guard judges them).
pub const TELEPORT_MIN_STEP_M: f64 = 5.0;
/// Fastest speed (m/s) of the car right after a teleport: LMU places it at rest (Gate 1: 0.03, 0.11 and
/// 0.55 m/s). A car still moving fast after a "jump" is a garbage position, not a teleport.
pub const TELEPORT_MAX_SPEED_MPS: f64 = 2.0;
/// Fastest speed (m/s) at which the car counts as stationary for a pit service (refuel, tyre change,
/// garage setup change). A serviced car stands still (on its jacks in the pit box).
pub const PIT_SERVICE_MAX_SPEED_MPS: f64 = 1.0;

/// Rate-limited channels a pit service can step (single channels).
pub const PIT_SERVICE_CHANNELS: &[ChannelId] = &[ch::FUEL, ch::VIRTUAL_ENERGY];
/// Rate-limited per-wheel channels a pit service can step (FL ids; all four wheels apply).
pub const PIT_SERVICE_WHEEL_CHANNELS: &[ChannelId] = &[
    ch::TYRE_PRESSURE_FL,
    ch::TYRE_TEMP_LEFT_FL,
    ch::TYRE_TEMP_MIDDLE_FL,
    ch::TYRE_TEMP_RIGHT_FL,
    ch::TYRE_LAYER_TEMP_LEFT_FL,
    ch::TYRE_LAYER_TEMP_MIDDLE_FL,
    ch::TYRE_LAYER_TEMP_RIGHT_FL,
    ch::TYRE_CARCASS_TEMP_FL,
    ch::TYRE_WEAR_FL,
    ch::TYRE_TEMP_INNER_FL,
    ch::TYRE_TEMP_OUTER_FL,
];

/// Every channel whose rate history a pit service resets (singles plus the four wheels of each group).
pub fn pit_service_channels() -> Vec<ChannelId> {
    let mut ids = PIT_SERVICE_CHANNELS.to_vec();
    for &fl in PIT_SERVICE_WHEEL_CHANNELS {
        ids.extend(Wheel::ALL.map(|wheel| wheel_id(fl, wheel)));
    }
    ids
}

/// The facts about one published sample the detector needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarSample {
    /// Player telemetry ET (s).
    pub et_s: f64,
    /// World position (m).
    pub pos: [f64; 3],
    /// Vehicle speed (m/s); NaN when unknown.
    pub speed_mps: f64,
    /// In the pit area: pit-lane bit of `mCurrentSector`, or the player's scoring row says `mInPits` or
    /// `mInGarageStall`.
    pub in_pit_area: bool,
}

/// A recognised discontinuity of the car state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Discontinuity {
    /// The car jumped further than it can move, came to rest and is in the pit area (garage release or
    /// return): every rate history is void.
    Teleport,
    /// The car was (re)entered into realtime (put on track from the monitor): every rate history is void.
    RealtimeEntry,
    /// The car stands still in the pit area: fuel, energy and tyre channels may step (service).
    PitService,
}

/// Classifies each published sample against the previous one.
#[derive(Debug, Clone, Default)]
pub struct DiscontinuityDetector {
    prev: Option<CarSample>,
    /// `prev` was itself classified as a [`Discontinuity::Teleport`] (the car arrived there by a jump).
    prev_teleport: bool,
    prev_in_realtime: Option<bool>,
    /// A realtime entry was seen since the last published sample.
    realtime_entered: bool,
}

impl DiscontinuityDetector {
    /// Feeds `scoringInfo.mInRealtime` of every snapshot (with or without player telemetry), so that an entry
    /// is seen even when no telemetry is published outside realtime.
    pub fn note_realtime(&mut self, in_realtime: bool) {
        if self.prev_in_realtime == Some(false) && in_realtime {
            self.realtime_entered = true;
        }
        self.prev_in_realtime = Some(in_realtime);
    }

    /// Classifies a new published sample. `max_motion_mps` is the fastest plausible motion (the registry's
    /// position rate bound): a longer step in the time available is not motion.
    pub fn observe(&mut self, sample: CarSample, max_motion_mps: f64) -> Option<Discontinuity> {
        let entered = std::mem::take(&mut self.realtime_entered);
        let teleport = self
            .prev
            .is_some_and(|prev| is_teleport(&prev, &sample, max_motion_mps));
        if sample.et_s.is_finite() && sample.pos.iter().all(|v| v.is_finite()) {
            self.prev = Some(sample);
            self.prev_teleport = teleport;
        }
        if teleport {
            Some(Discontinuity::Teleport)
        } else if entered {
            Some(Discontinuity::RealtimeEntry)
        } else if is_pit_service(&sample) {
            Some(Discontinuity::PitService)
        } else {
            None
        }
    }
}

/// Euclidean distance between two positions (m).
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f64>()
        .sqrt()
}

/// A teleport: a step longer than [`TELEPORT_MIN_STEP_M`] and than any motion in the elapsed time, with the
/// car at rest afterwards and in the pit area. All three conditions are needed, so a garbage position (car
/// still moving, or out on track) is never accepted as a teleport.
pub fn is_teleport(prev: &CarSample, next: &CarSample, max_motion_mps: f64) -> bool {
    let dt = next.et_s - prev.et_s;
    let step = distance(prev.pos, next.pos);
    let impossible_as_motion = dt.is_finite() && dt > 0.0 && step > max_motion_mps * dt;
    let at_rest = (0.0..=TELEPORT_MAX_SPEED_MPS).contains(&next.speed_mps);
    step.is_finite()
        && step > TELEPORT_MIN_STEP_M
        && impossible_as_motion
        && at_rest
        && next.in_pit_area
}

/// Largest ET step-back (s) that can accompany a garage teleport (A-LMU-031 kind (b)). Gate 1 (LMU 1.4.2.0)
/// saw 0.01-0.03 s (pitstop1: ET 315.8304 -> 315.8200, one sample after a 19.2 m jump into the garage); a
/// larger step-back is never explained as a teleport.
pub const TELEPORT_ET_STEP_BACK_MAX_S: f64 = 0.05;

/// A teleport whose ET stepped back a little ([`TELEPORT_ET_STEP_BACK_MAX_S`]) instead of advancing: the
/// position step is longer than [`TELEPORT_MIN_STEP_M`], the car is at rest afterwards and in the pit area.
/// No motion test is possible (no positive time step), so the step-back size is the extra corroboration
/// and the caller must also check that the player and session did not change.
///
/// This is the jump and the step-back on the same sample. The order seen on real data (Gate 1 pitstop1) is
/// the jump first, with ET still advancing, and the step-back on the next sample: see
/// [`is_step_back_after_teleport`].
pub fn is_teleport_step_back(prev: &CarSample, next: &CarSample) -> bool {
    let back = prev.et_s - next.et_s;
    let step = distance(prev.pos, next.pos);
    let at_rest = (0.0..=TELEPORT_MAX_SPEED_MPS).contains(&next.speed_mps);
    back.is_finite()
        && back > 0.0
        && back <= TELEPORT_ET_STEP_BACK_MAX_S
        && step.is_finite()
        && step > TELEPORT_MIN_STEP_M
        && at_rest
        && next.in_pit_area
}

/// A small ET step-back ([`TELEPORT_ET_STEP_BACK_MAX_S`]) on the sample right after a garage teleport:
/// `teleported` is that teleport sample (the caller checks it was classified
/// [`Discontinuity::Teleport`]), and `next` stays where the teleport put the car (a step of at most
/// [`TELEPORT_MIN_STEP_M`]), at rest and in the pit area.
///
/// Gate 1 pitstop1 (LMU 1.4.2.0), return to the garage: ET 315.8025 -> 315.8304 with a 19.17 m jump (speed
/// 0.000 m/s, pit-lane bit set), then 1.3 ms later ET 315.8200 (back 0.0104 s), 0.00 m away, 0.004 m/s,
/// pit-lane bit set; ET then advances again from 315.8400. A step-back that is not right after a teleport,
/// that moves the car again, that leaves the pit area or is larger than the bound is not this.
pub fn is_step_back_after_teleport(teleported: &CarSample, next: &CarSample) -> bool {
    let back = teleported.et_s - next.et_s;
    let step = distance(teleported.pos, next.pos);
    let at_rest = (0.0..=TELEPORT_MAX_SPEED_MPS).contains(&next.speed_mps);
    back.is_finite()
        && back > 0.0
        && back <= TELEPORT_ET_STEP_BACK_MAX_S
        && step.is_finite()
        && step <= TELEPORT_MIN_STEP_M
        && at_rest
        && next.in_pit_area
}

impl DiscontinuityDetector {
    /// `true` when `next` is a garage teleport with a small ET step-back relative to the last published
    /// sample (see [`is_teleport_step_back`]).
    pub fn is_step_back_teleport(&self, next: &CarSample) -> bool {
        self.prev
            .is_some_and(|prev| is_teleport_step_back(&prev, next))
    }

    /// `true` when the last published sample was a [`Discontinuity::Teleport`] and `next` steps ET back a
    /// little right after it without moving (see [`is_step_back_after_teleport`]).
    pub fn is_step_back_after_last_teleport(&self, next: &CarSample) -> bool {
        self.prev_teleport
            && self
                .prev
                .is_some_and(|prev| is_step_back_after_teleport(&prev, next))
    }

    /// Records the car's new place after a recognised step-back teleport (the sample itself is not
    /// published), so that the next sample is compared with the post-teleport position.
    pub fn accept_step_back(&mut self, sample: CarSample) {
        if sample.pos.iter().all(|v| v.is_finite()) {
            // Keep the larger ET of the time base: only the position moves.
            let et_s = self.prev.map_or(sample.et_s, |p| p.et_s.max(sample.et_s));
            self.prev = Some(CarSample { et_s, ..sample });
            // The teleport is consumed: a further step-back is the caller's open step-back, not a new one.
            self.prev_teleport = false;
        }
    }
}

/// Raw `mGamePhase` of a finished session (`GamePhase::SessionOver`).
pub const LMU_GAME_PHASE_SESSION_OVER: u8 = 8;

/// A **session-end ET rewind** (A-LMU-031): an ET regression with the session over (`mGamePhase` 8), the
/// player out of realtime (in the garage / monitor) and the new ET at or after the session end time.
///
/// Gate 1 stint1 practice (LMU 1.4.2.0): the session ended at `mEndET` 605 s, the driver kept lapping to
/// ET 652 s and returned to the garage; after a 19.5 s stall LMU rewound ET to 606.02 s (end + 1.02 s) and
/// sent `EndSession` + `Unload` 0.6 s later. It is the same session being closed, not a restart: the time
/// base is void (reset) but no new session begins. A regression before the session end, in realtime or
/// while the session runs is never this (it stays a restart, or an unexplained fault).
pub fn is_session_end_rewind(
    game_phase: u8,
    in_realtime: bool,
    end_et_s: Option<f64>,
    prev_et_s: f64,
    new_et_s: f64,
) -> bool {
    game_phase == LMU_GAME_PHASE_SESSION_OVER
        && !in_realtime
        && new_et_s < prev_et_s
        && end_et_s.is_some_and(|end| end.is_finite() && end > 0.0 && new_et_s >= end)
}

/// A pit service window: stationary in the pit area.
pub fn is_pit_service(sample: &CarSample) -> bool {
    sample.in_pit_area && (0.0..=PIT_SERVICE_MAX_SPEED_MPS).contains(&sample.speed_mps)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry's position rate bound (m/s) as the normalizer passes it.
    const MAX_MOTION_MPS: f64 = 200.0;

    fn sample(et_s: f64, x: f64, speed_mps: f64, in_pit_area: bool) -> CarSample {
        CarSample {
            et_s,
            pos: [x, 0.0, 0.0],
            speed_mps,
            in_pit_area,
        }
    }

    #[test]
    fn garage_release_and_return_are_teleports() {
        // Gate 1: released from the garage (9.2 m in 12.5 ms, 0.03 m/s, pit-lane bit set).
        let mut d = DiscontinuityDetector::default();
        assert_eq!(
            d.observe(sample(62.180, 0.0, 0.0, true), MAX_MOTION_MPS),
            Some(Discontinuity::PitService),
            "stationary in the garage stall"
        );
        assert_eq!(
            d.observe(sample(62.1925, 9.23, 0.03, true), MAX_MOTION_MPS),
            Some(Discontinuity::Teleport)
        );
        // Driving down the pit lane is neither.
        assert_eq!(
            d.observe(sample(62.2025, 9.39, 16.0, true), MAX_MOTION_MPS),
            None
        );
        // Return to the garage: 652 m in 7.5 ms, at rest, pit-lane bit set.
        let mut r = DiscontinuityDetector::default();
        r.observe(sample(397.0325, 0.0, 0.0, false), MAX_MOTION_MPS);
        assert_eq!(
            r.observe(sample(397.040, 652.0, 0.11, true), MAX_MOTION_MPS),
            Some(Discontinuity::Teleport)
        );
    }

    #[test]
    fn uncorroborated_jumps_are_not_teleports() {
        let prev = sample(100.0, 0.0, 60.0, false);
        // On track (not in the pit area): a garbage position, rejected by the rate guard.
        assert!(!is_teleport(
            &prev,
            &sample(100.01, 652.0, 0.1, false),
            MAX_MOTION_MPS
        ));
        // Still moving fast after the jump (e.g. another car's position): not a teleport.
        assert!(!is_teleport(
            &prev,
            &sample(100.01, 652.0, 60.0, true),
            MAX_MOTION_MPS
        ));
        // Short steps are never teleports, even if fast.
        assert!(!is_teleport(
            &prev,
            &sample(100.01, 4.0, 0.1, true),
            MAX_MOTION_MPS
        ));
        // Plausible motion over a long gap (2 s at 80 m/s = 160 m) is motion, not a teleport.
        assert!(!is_teleport(
            &prev,
            &sample(102.0, 160.0, 0.5, true),
            MAX_MOTION_MPS
        ));
        // No time step, or non-finite inputs: nothing can be assessed.
        assert!(!is_teleport(
            &prev,
            &sample(100.0, 652.0, 0.1, true),
            MAX_MOTION_MPS
        ));
        assert!(!is_teleport(
            &prev,
            &sample(100.01, f64::NAN, 0.1, true),
            MAX_MOTION_MPS
        ));
        assert!(!is_teleport(
            &prev,
            &sample(100.01, 652.0, f64::NAN, true),
            MAX_MOTION_MPS
        ));
    }

    /// Synthetic: the jump into the garage and the ET step-back on the same sample (not seen on real data;
    /// the real order is covered by `step_back_right_after_a_garage_teleport_is_a_teleport_step_back`).
    #[test]
    fn small_et_step_back_with_a_garage_jump_is_a_teleport_step_back() {
        let prev = sample(315.830, 0.0, 0.0, true);
        assert!(is_teleport_step_back(
            &prev,
            &sample(315.820, 19.6, 0.0, true)
        ));
        // Too large a step-back, no jump, moving, on track, or no step-back at all: not it.
        assert!(!is_teleport_step_back(
            &prev,
            &sample(315.700, 19.6, 0.0, true)
        ));
        assert!(!is_teleport_step_back(
            &prev,
            &sample(315.820, 1.0, 0.0, true)
        ));
        assert!(!is_teleport_step_back(
            &prev,
            &sample(315.820, 19.6, 30.0, true)
        ));
        assert!(!is_teleport_step_back(
            &prev,
            &sample(315.820, 19.6, 0.0, false)
        ));
        assert!(!is_teleport_step_back(
            &prev,
            &sample(315.840, 19.6, 0.0, true)
        ));
        let mut d = DiscontinuityDetector::default();
        d.observe(prev, MAX_MOTION_MPS);
        let next = sample(315.820, 19.6, 0.0, true);
        assert!(d.is_step_back_teleport(&next));
        d.accept_step_back(next);
        assert!(
            !d.is_step_back_teleport(&sample(315.825, 19.6, 0.0, true)),
            "the post-teleport position is the new reference"
        );
    }

    /// Gate 1 pitstop1 (real sample, positions relative to the pre-jump place): the 19.17 m jump arrives
    /// with ET advancing, the step-back of 0.0104 s on the next sample without moving.
    #[test]
    fn step_back_right_after_a_garage_teleport_is_a_teleport_step_back() {
        let before = sample(315.8025, 0.0, 0.169, true);
        let teleported = sample(315.8304, 19.17, 0.000, true);
        let back = sample(315.8200, 19.17, 0.004, true);
        let mut d = DiscontinuityDetector::default();
        d.observe(before, MAX_MOTION_MPS);
        assert!(
            !d.is_step_back_after_last_teleport(&back),
            "no teleport yet"
        );
        assert_eq!(
            d.observe(teleported, MAX_MOTION_MPS),
            Some(Discontinuity::Teleport)
        );
        assert!(
            !d.is_step_back_teleport(&back),
            "no jump on the step-back sample itself"
        );
        assert!(d.is_step_back_after_last_teleport(&back));
        d.accept_step_back(back);
        assert!(
            !d.is_step_back_after_last_teleport(&sample(315.8250, 19.17, 0.0, true)),
            "the teleport is consumed by the first step-back"
        );

        assert!(is_step_back_after_teleport(&teleported, &back));
        // Too large a step-back, moved again, moving, out of the pit area, or no step-back: not it.
        assert!(!is_step_back_after_teleport(
            &teleported,
            &sample(315.70, 19.17, 0.0, true)
        ));
        assert!(!is_step_back_after_teleport(
            &teleported,
            &sample(315.82, 30.0, 0.0, true)
        ));
        assert!(!is_step_back_after_teleport(
            &teleported,
            &sample(315.82, 19.17, 30.0, true)
        ));
        assert!(!is_step_back_after_teleport(
            &teleported,
            &sample(315.82, 19.17, 0.0, false)
        ));
        assert!(!is_step_back_after_teleport(
            &teleported,
            &sample(315.84, 19.17, 0.0, true)
        ));

        // The same step-back after an ordinary (non-teleport) sample is not explained.
        let mut plain = DiscontinuityDetector::default();
        plain.observe(sample(315.8304, 19.17, 0.0, true), MAX_MOTION_MPS);
        assert!(!plain.is_step_back_after_last_teleport(&back));
    }

    #[test]
    fn realtime_entry_and_pit_service() {
        let mut d = DiscontinuityDetector::default();
        d.note_realtime(true);
        assert_eq!(
            d.observe(sample(1.0, 0.0, 50.0, false), MAX_MOTION_MPS),
            None,
            "already in realtime when first seen: no entry"
        );
        d.note_realtime(false);
        d.note_realtime(true);
        assert_eq!(
            d.observe(sample(2.0, 10.0, 0.0, true), MAX_MOTION_MPS),
            Some(Discontinuity::RealtimeEntry)
        );
        assert_eq!(
            d.observe(sample(2.01, 10.0, 0.0, true), MAX_MOTION_MPS),
            Some(Discontinuity::PitService),
            "the entry is reported once"
        );
        // Creeping in the pit lane above the service speed, or stationary on track: no service.
        assert!(!is_pit_service(&sample(3.0, 0.0, 1.5, true)));
        assert!(!is_pit_service(&sample(3.0, 0.0, 0.0, false)));
        assert!(!is_pit_service(&sample(3.0, 0.0, f64::NAN, true)));
    }

    #[test]
    fn service_channels_cover_fuel_energy_and_all_four_tyres() {
        let ids = pit_service_channels();
        assert_eq!(
            ids.len(),
            PIT_SERVICE_CHANNELS.len() + 4 * PIT_SERVICE_WHEEL_CHANNELS.len()
        );
        for id in [
            ch::FUEL,
            ch::VIRTUAL_ENERGY,
            ch::TYRE_WEAR_RR,
            ch::TYRE_PRESSURE_FR,
        ] {
            assert!(ids.contains(&id), "{id:?}");
        }
        // Every listed channel is rate limited (otherwise listing it would be meaningless).
        for id in &ids {
            let def = telemetry_model::registry::by_id(*id).expect("registered");
            assert!(def.max_rate_per_s.is_some(), "{}", def.name);
        }
        assert!(
            !ids.contains(&ch::BRAKE_TEMP_FL),
            "a pit stop does not reset brakes"
        );
    }

    /// Gate 1 stint1 practice: ET 653.025 → 606.020 s with the session over at 605 s, out of realtime.
    #[test]
    fn session_end_rewind_needs_session_over_out_of_realtime_and_an_et_past_the_end() {
        let over = LMU_GAME_PHASE_SESSION_OVER;
        assert!(is_session_end_rewind(
            over,
            false,
            Some(605.0),
            653.025,
            606.02
        ));
        // Exactly at the end time still counts.
        assert!(is_session_end_rewind(
            over,
            false,
            Some(605.0),
            653.0,
            605.0
        ));
        // Running session (green flag = 5), in realtime, before the end, unset end or no regression: not it.
        assert!(!is_session_end_rewind(5, false, Some(605.0), 653.0, 606.0));
        assert!(!is_session_end_rewind(
            over,
            true,
            Some(605.0),
            653.0,
            606.0
        ));
        assert!(!is_session_end_rewind(over, false, Some(605.0), 653.0, 4.5));
        assert!(!is_session_end_rewind(over, false, None, 653.0, 606.0));
        assert!(!is_session_end_rewind(
            over,
            false,
            Some(f64::NAN),
            653.0,
            606.0
        ));
        assert!(!is_session_end_rewind(
            over,
            false,
            Some(605.0),
            606.0,
            653.0
        ));
    }
}

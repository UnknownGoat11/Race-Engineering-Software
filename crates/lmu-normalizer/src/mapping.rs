//! LMU raw fields → canonical channels (docs/LMU_CHANNEL_MAP.md is the specification).
//!
//! This is the only place an LMU field is read into a canonical channel. Conversions use
//! `telemetry_model::units::convert` exclusively; sign conventions come from [`crate::convention`]; the tyre
//! side mapping from [`crate::tyre`]; unit hazards from [`crate::detect`]. Sentinels ("no time", −1, the
//! INT32_MAX lap limit…) leave the channel at its "no data" default; values that cannot be represented become
//! `Invalid`; capability-gated channels become `NotSupported`. Nothing is ever replaced by 0.

use std::collections::BTreeMap;
use std::f64::consts::TAU;

use lmu_reader::{RawGeneric, RawScoringInfo, RawTelemetry, RawVehicleScoring, TelemetryHeader};
use telemetry_model::ch;
use telemetry_model::events::Severity;
use telemetry_model::registry::Wheel;
use telemetry_model::scoring::{CompetitorEntry, VehicleCapabilities, lmu_max_laps, lmu_time};
use telemetry_model::units::convert;
use telemetry_model::{ChannelId, Quality};

use crate::convention;
use crate::detect::{
    BrakeObservation, BrakePressureGuard, BrakeTempUnitDetector, BrakeUnitTransition, FillState,
    FractionScale, FractionScaleDetector, KelvinLatch, Odometer, StuckValueDetector,
    TimeOfDayDetector, TurboReading, TurboUnitDetector, verified_brake_temp_unit,
    verified_virtual_energy_scale,
};
use crate::notice::{DiagnosticNotice, NoticeCode, NoticeLog};
use crate::standings::{finite, lap_start_et, non_negative, qualification, text};
use crate::tyre::lcr_to_edges;
use crate::writer::FrameWriter;

/// Seconds per minute (rpm → rad/s).
const SECONDS_PER_MINUTE: f64 = 60.0;
/// Highest zero-based sector index.
const MAX_SECTOR_INDEX: u32 = 2;
/// Sectors per lap (scoring `mSector` remap modulus).
const SECTORS_PER_LAP: i8 = 3;
/// Scoring `mSector` → zero-based S1 index offset: `(mSector + 2) mod 3` (0 = S3 → 2).
const SCORING_SECTOR_OFFSET: i8 = 2;
/// `mGamePhase` value for full-course yellow / safety car.
const GAME_PHASE_FCY: u8 = 6;
/// `mYellowFlagState` values meaning a full-course caution is in progress (pending … resume).
const YELLOW_STATE_FCY: (i8, i8) = (1, 6);
/// `mFlag` value for the blue flag.
const CAR_FLAG_BLUE: u8 = 6;
/// Maximum |telemetry ET − scoring ET| over which lap progress is extrapolated from the last scoring
/// update (s). Beyond it the anchor is too old (scoring stalled) and progress is withheld.
pub const MAX_PROGRESS_EXTRAPOLATION_S: f64 = 1.0;
/// Largest lap progress value (the interval is [0, 1)).
const PROGRESS_MAX: f64 = 1.0 - f64::EPSILON;

/// Per-wheel channel id: per-wheel groups use a stride of 4 (`FL, FR, RL, RR` = `base + 0..=3`).
pub(crate) fn wheel_id(front_left: ChannelId, wheel: Wheel) -> ChannelId {
    // Wheel::index() is 0..=3.
    ChannelId(front_left.0 + wheel.index() as u16)
}

/// Normalizer-derived channels (formula ids in the registry) computed here. Everything else marked
/// `Calculated` in the registry belongs to `race-engines` or is experimental.
pub const DERIVED_HERE: &[ChannelId] = &[
    ch::SPEED,
    ch::SPEED_FORWARD,
    ch::STEERING_ANGLE,
    ch::ENGINE_POWER,
    ch::FUEL_FRACTION,
    ch::G_LONG,
    ch::G_LAT,
    ch::G_VERT,
    ch::G_COMBINED,
    ch::YAW_RATE,
    ch::PITCH_RATE,
    ch::ROLL_RATE,
    ch::LAP_TIME,
    ch::LAST_SECTOR2,
    ch::LAST_SECTOR3,
    ch::BEST_SECTOR2,
    ch::SESSION_TIME_REMAINING_CALC,
    ch::CLASS_POSITION,
    ch::DISTANCE,
    ch::LAP_PROGRESS,
    ch::SECTOR,
    ch::SECTOR_SCORING,
    ch::HEADING,
    ch::MAP_X,
    ch::MAP_Y,
    ch::FULL_COURSE_YELLOW,
    ch::BLUE_FLAG,
    ch::IN_PIT_LANE,
];

/// Per-wheel derived channels computed here (FL ids).
pub const DERIVED_HERE_PER_WHEEL: &[ChannelId] = &[
    ch::TYRE_TEMP_INNER_FL,
    ch::TYRE_TEMP_OUTER_FL,
    ch::TYRE_TEMP_AVG_FL,
    ch::TYRE_TEMP_SPREAD_FL,
    ch::WHEEL_SPEED_FL,
];

/// When a value that has not changed since the first reading is impossible before the travel evidence
/// ([`crate::detect::STUCK_MIN_TRAVEL_M`]) is in (A-LMU-065).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FillRule {
    /// This exact value is impossible even on a stationary car (a 0 m ride height).
    ImpossibleAtRest(f64),
    /// This exact value is impossible once the car moves faster than [`FILL_MOVING_SPEED_MPS`] (0 N of drag,
    /// or exactly the standard atmosphere as boost pressure under way); plausible at rest.
    ImpossibleWhileMoving(f64),
}

/// Speed (m/s) above which an aerodynamic force of exactly 0 N, or a boost pressure stuck at exactly the
/// standard atmosphere, is not a measurement (drag is already tens of newtons at this speed).
pub const FILL_MOVING_SPEED_MPS: f64 = 2.0;
/// ISA standard atmosphere (Pa): the value LMU 1.4.2.0 initialises `mTurboBoostPressure` with.
pub const STANDARD_ATMOSPHERE_PA: f64 = 101_325.0;

/// Continuous channels LMU 1.4.2.0 leaves at their first value on some cars (A-LMU-065; Gate 1, Hypercar:
/// exactly 0, or exactly 101,325 Pa, for the whole session while the car lapped Monza). Any of them that
/// keeps its first value over [`crate::detect::STUCK_MIN_TRAVEL_M`] of driving is withheld; the rule says
/// which first value is impossible even earlier.
pub const FILL_CHECKED_CHANNELS: &[(ChannelId, FillRule)] = &[
    // A car resting on its wheels never has a 0 m ride height or a front wing touching the ground.
    (ch::RIDE_HEIGHT_FRONT, FillRule::ImpossibleAtRest(0.0)),
    (ch::RIDE_HEIGHT_REAR, FillRule::ImpossibleAtRest(0.0)),
    (ch::FRONT_WING_HEIGHT, FillRule::ImpossibleAtRest(0.0)),
    (ch::DRAG, FillRule::ImpossibleWhileMoving(0.0)),
    (ch::DOWNFORCE_FRONT, FillRule::ImpossibleWhileMoving(0.0)),
    (ch::DOWNFORCE_REAR, FillRule::ImpossibleWhileMoving(0.0)),
    // Absolute boost equals ambient with the engine off; exactly the ISA value under way is LMU's default.
    (
        ch::TURBO_BOOST,
        FillRule::ImpossibleWhileMoving(STANDARD_ATMOSPHERE_PA),
    ),
];

/// Session-scoped detectors (reset on a session, track or vehicle change and on an ET regression).
#[derive(Debug, Default)]
pub struct SessionDetectors {
    pub brake_unit: BrakeTempUnitDetector,
    pub brake_pressure: BrakePressureGuard,
    pub virtual_energy: FractionScaleDetector,
    pub soc: FractionScaleDetector,
    pub optimal_temp: [KelvinLatch; 4],
    pub motor_temp: KelvinLatch,
    pub motor_water_temp: KelvinLatch,
    pub turbo: TurboUnitDetector,
    pub time_of_day: TimeOfDayDetector,
    /// Distance driven in the session (evidence for [`FILL_CHECKED_CHANNELS`]).
    pub odometer: Odometer,
    /// Fill detectors of [`FILL_CHECKED_CHANNELS`] (bounded by that table).
    pub fill: BTreeMap<ChannelId, StuckValueDetector>,
}

impl SessionDetectors {
    /// The fill state of a [`FILL_CHECKED_CHANNELS`] channel (`Unknown` for any other channel).
    pub fn fill_state(&self, id: ChannelId) -> FillState {
        self.fill
            .get(&id)
            .map_or(FillState::Unknown, |d| d.state(self.odometer.travelled_m()))
    }
}

/// Inputs of one mapping.
pub struct MapInput<'a> {
    pub telemetry: &'a RawTelemetry,
    pub header: &'a TelemetryHeader,
    pub generic: &'a RawGeneric,
    pub info: &'a RawScoringInfo,
    /// The player's scoring row (`mIsPlayer` joined by `mID`), if determined.
    pub player_row: Option<&'a RawVehicleScoring>,
    /// The player's derived class place, if the standings are consistent.
    pub class_place: Option<u8>,
    pub caps: VehicleCapabilities,
}

/// Mutable state the mapping uses.
pub struct MapState<'a> {
    pub det: &'a mut SessionDetectors,
    pub notice_log: &'a mut NoticeLog,
    pub notices: &'a mut Vec<DiagnosticNotice>,
}

impl MapState<'_> {
    fn raise(&mut self, notice: DiagnosticNotice) {
        self.notice_log.raise(notice, self.notices);
    }
}

/// Maps one player telemetry sample (plus the current scoring/generic blocks) into `w`.
pub fn map_frame(input: &MapInput<'_>, st: &mut MapState<'_>, w: &mut FrameWriter<'_>) {
    let t = input.telemetry;
    st.det
        .odometer
        .observe(t.elapsed_time, convention::speed_mps(t.local_vel));
    map_generic(input.generic, w);
    map_header(input.header, w);
    map_inputs(t, w);
    map_identity(t, w);
    map_aids(t, input.caps, w);
    map_powertrain(t, st, w);
    map_dynamics(t, w);
    map_chassis(t, st, w);
    map_tyres(t, input.caps, st, w);
    map_brakes(input, st, w);
    map_energy(input, st, w);
    map_timing(t, input.info, w);
    map_track_position(t, w);
    map_session(input.info, st, w);
    map_player_scoring(input, w);
}

/// Publishes `value` (the converted form of `raw`) unless the fill detector of `id` says it is not a
/// measurement (A-LMU-065): a value still bit-identical to its first reading after
/// [`crate::detect::STUCK_MIN_TRAVEL_M`] of driving, or a first value its [`FillRule`] makes impossible now,
/// is withheld as `UNAVAILABLE`. The first change publishes the channel again for the session.
fn fill_checked(
    st: &mut MapState<'_>,
    w: &mut FrameWriter<'_>,
    (id, raw, value): (ChannelId, f64, f64),
    speed_mps: f64,
) {
    let rule = FILL_CHECKED_CHANNELS
        .iter()
        .find(|(rule_id, _)| *rule_id == id)
        .map(|&(_, rule)| rule);
    let travelled = st.det.odometer.travelled_m();
    let state = st.det.fill.entry(id).or_default().observe(raw, travelled);
    let same = |v: f64| v.to_bits() == raw.to_bits();
    let moving = speed_mps > FILL_MOVING_SPEED_MPS;
    let reason = match (state, rule) {
        (FillState::NotFilled, _) => Some(
            "value never changed from its first reading while the car drove: not filled by LMU for this \
             car/version, withheld (UNAVAILABLE) until it changes (A-LMU-065)",
        ),
        (FillState::Initial, Some(FillRule::ImpossibleAtRest(v))) if same(v) => Some(
            "first value is physically impossible even at rest (not filled by LMU?): withheld \
             (UNAVAILABLE) until it changes (A-LMU-065)",
        ),
        (FillState::Initial, Some(FillRule::ImpossibleWhileMoving(v))) if same(v) && moving => {
            Some(
                "unchanged first value is physically impossible with the car moving (not filled by LMU?): \
             withheld (UNAVAILABLE) until it changes (A-LMU-065)",
            )
        }
        _ => None,
    };
    match reason {
        Some(message) => {
            st.raise(
                DiagnosticNotice::new(
                    NoticeCode::ChannelNotFilled,
                    Severity::Notice,
                    Some(id),
                    message,
                )
                .with_raw(raw),
            );
            w.quality(id, Quality::Unavailable);
        }
        None => w.float(id, value),
    }
}

// ---------------------------------------------------------------------------------------------------------
// Generic block and telemetry header
// ---------------------------------------------------------------------------------------------------------

fn map_generic(g: &RawGeneric, w: &mut FrameWriter<'_>) {
    w.int(ch::GAME_VERSION, i64::from(g.game_version));
    w.float(ch::FFB_TORQUE, f64::from(g.ffb_torque));
    w.int(ch::OPTIONS_LOCATION, i64::from(g.app_info.options_location));
    w.text(ch::OPTIONS_PAGE, g.app_info.options_page.as_str());
}

fn map_header(h: &TelemetryHeader, w: &mut FrameWriter<'_>) {
    w.int(ch::NUM_VEHICLES_TELEMETRY, i64::from(h.active_vehicles));
    w.boolean(ch::PLAYER_HAS_VEHICLE, h.player_has_vehicle != 0);
    w.int(ch::PLAYER_SLOT_INDEX, i64::from(h.player_vehicle_idx));
}

// ---------------------------------------------------------------------------------------------------------
// Driver inputs (§4)
// ---------------------------------------------------------------------------------------------------------

fn map_inputs(t: &RawTelemetry, w: &mut FrameWriter<'_>) {
    w.float(ch::THROTTLE, t.unfiltered_throttle);
    w.float(ch::BRAKE, t.unfiltered_brake);
    w.float(ch::CLUTCH, t.unfiltered_clutch);
    w.float(ch::STEERING, t.unfiltered_steering);
    w.float(ch::THROTTLE_FILTERED, t.filtered_throttle);
    w.float(ch::BRAKE_FILTERED, t.filtered_brake);
    w.float(ch::CLUTCH_FILTERED, t.filtered_clutch);
    w.float(ch::STEERING_FILTERED, t.filtered_steering);
    w.float(ch::STEERING_TORQUE, t.steering_shaft_torque);
    // A-LMU-037: the ranges are assumed to be full lock-to-lock in degrees. A zero range is "unknown"
    // (not set by the car/game), so it is published as Unavailable rather than as an invalid zero.
    let range_deg = f64::from(t.physical_steering_wheel_range);
    let range_known = range_deg.is_finite() && range_deg > 0.0;
    w.opt_float(
        ch::STEERING_RANGE_PHYSICAL,
        range_known.then(|| convert::deg_to_rad(range_deg)),
    );
    let visual_deg = f64::from(t.visual_steering_wheel_range);
    let visual_known = visual_deg.is_finite() && visual_deg > 0.0;
    w.opt_float(
        ch::STEERING_RANGE_VISUAL,
        visual_known.then(|| convert::deg_to_rad(visual_deg)),
    );
    // Steering wheel angle = input (−1..1) × half the lock-to-lock range.
    let angle = range_known.then(|| t.unfiltered_steering * convert::deg_to_rad(range_deg / 2.0));
    w.opt_float(ch::STEERING_ANGLE, angle);
}

// ---------------------------------------------------------------------------------------------------------
// Vehicle identity (§2)
// ---------------------------------------------------------------------------------------------------------

fn map_identity(t: &RawTelemetry, w: &mut FrameWriter<'_>) {
    w.int(ch::VEHICLE_SLOT_ID, i64::from(t.id));
    w.text(ch::VEHICLE_NAME, t.vehicle_name.as_str());
    w.text(ch::VEHICLE_MODEL, t.vehicle_model.as_str());
    w.int(ch::VEHICLE_CLASS, i64::from(t.vehicle_class));
    w.int(ch::VEHICLE_CHAMPIONSHIP, i64::from(t.vehicle_championship));
    w.int(ch::GEARS_MAX, i64::from(t.max_gears));
    w.float(ch::RPM_MAX, t.engine_max_rpm);
    w.boolean(ch::HEADLIGHTS, t.headlights);
    w.int(ch::WIPER_STATE, i64::from(t.wiper_state));
    // Damage (§13).
    for (i, &dent) in t.dent_severity.iter().enumerate() {
        // The 8 dent channels are consecutive ids.
        w.int(ChannelId(ch::DENT_SEVERITY_0.0 + i as u16), i64::from(dent));
    }
    w.boolean(ch::PARTS_DETACHED, t.detached);
    w.float(ch::LAST_IMPACT_TIME, t.last_impact_et);
    w.float(ch::LAST_IMPACT_MAGNITUDE, t.last_impact_magnitude);
    w.float(ch::LAST_IMPACT_POS_X, t.last_impact_pos.x);
    w.float(ch::LAST_IMPACT_POS_Y, t.last_impact_pos.y);
    w.float(ch::LAST_IMPACT_POS_Z, t.last_impact_pos.z);
}

// ---------------------------------------------------------------------------------------------------------
// Driver aids and settings (§8)
// ---------------------------------------------------------------------------------------------------------

/// A setting gated by its maximum: a maximum of 0 means the car has no such adjustable system.
fn gated_setting(w: &mut FrameWriter<'_>, id: ChannelId, value: u8, max_id: ChannelId, max: u8) {
    w.int(max_id, i64::from(max));
    if max == 0 {
        w.quality(id, Quality::NotSupported);
    } else {
        w.int(id, i64::from(value));
    }
}

fn map_aids(t: &RawTelemetry, caps: VehicleCapabilities, w: &mut FrameWriter<'_>) {
    gated_setting(w, ch::TC_LEVEL, t.tc, ch::TC_LEVEL_MAX, t.tc_max);
    gated_setting(w, ch::TC_SLIP, t.tc_slip, ch::TC_SLIP_MAX, t.tc_slip_max);
    gated_setting(w, ch::TC_CUT, t.tc_cut, ch::TC_CUT_MAX, t.tc_cut_max);
    gated_setting(w, ch::ABS_LEVEL, t.abs, ch::ABS_LEVEL_MAX, t.abs_max);
    gated_setting(
        w,
        ch::BRAKE_MIGRATION,
        t.migration,
        ch::BRAKE_MIGRATION_MAX,
        t.migration_max,
    );
    gated_setting(
        w,
        ch::MOTOR_MAP,
        t.motor_map,
        ch::MOTOR_MAP_MAX,
        t.motor_map_max,
    );
    gated_setting(
        w,
        ch::ARB_FRONT,
        t.front_anti_sway,
        ch::ARB_FRONT_MAX,
        t.front_anti_sway_max,
    );
    gated_setting(
        w,
        ch::ARB_REAR,
        t.rear_anti_sway,
        ch::ARB_REAR_MAX,
        t.rear_anti_sway_max,
    );
    // Intervention booleans exist only for cars with the system.
    match caps.supports_tc_intervention {
        Some(false) => w.quality(ch::TC_ACTIVE, Quality::NotSupported),
        _ => w.boolean(ch::TC_ACTIVE, t.tc_active),
    }
    match caps.supports_abs_intervention {
        Some(false) => w.quality(ch::ABS_ACTIVE, Quality::NotSupported),
        _ => w.boolean(ch::ABS_ACTIVE, t.abs_active),
    }
    let flap_ids = [
        ch::FRONT_FLAP_ACTIVE,
        ch::REAR_FLAP_ACTIVE,
        ch::REAR_FLAP_LEGAL_STATUS,
    ];
    if caps.supports_rear_flap == Some(false) {
        flap_ids
            .into_iter()
            .for_each(|id| w.quality(id, Quality::NotSupported));
    } else {
        w.boolean(ch::FRONT_FLAP_ACTIVE, t.front_flap_activated != 0);
        w.boolean(ch::REAR_FLAP_ACTIVE, t.rear_flap_activated != 0);
        w.int(
            ch::REAR_FLAP_LEGAL_STATUS,
            i64::from(t.rear_flap_legal_status),
        );
    }
    w.float(ch::BRAKE_BIAS_REAR, t.rear_brake_bias);
}

// ---------------------------------------------------------------------------------------------------------
// Powertrain and fuel (§5, §6)
// ---------------------------------------------------------------------------------------------------------

fn map_powertrain(t: &RawTelemetry, st: &mut MapState<'_>, w: &mut FrameWriter<'_>) {
    w.int(ch::GEAR, i64::from(t.gear));
    w.float(ch::RPM, t.engine_rpm);
    w.float(ch::CLUTCH_RPM, t.clutch_rpm);
    w.float(ch::ENGINE_TORQUE, t.engine_torque);
    // Coolant and oil both exactly 0.000 °C: the engine model is not initialised (garage/reset transition,
    // A-LMU-062); a real cold engine reads near ambient, not two exact zeros.
    if t.engine_water_temp == 0.0 && t.engine_oil_temp == 0.0 {
        w.quality(ch::WATER_TEMP, Quality::Unavailable);
        w.quality(ch::OIL_TEMP, Quality::Unavailable);
    } else {
        w.float(ch::WATER_TEMP, t.engine_water_temp);
        w.float(ch::OIL_TEMP, t.engine_oil_temp);
    }
    w.boolean(ch::OVERHEATING, t.overheating);
    w.int(ch::IGNITION_STARTER, i64::from(t.ignition_starter));
    w.boolean(ch::ANTI_STALL, t.anti_stall_activated != 0);
    w.boolean(ch::SPEED_LIMITER, t.speed_limiter != 0);
    w.boolean(ch::SPEED_LIMITER_ACTIVE, t.speed_limiter_active);
    w.boolean(ch::SPEED_LIMITER_AVAILABLE, t.speed_limiter_available != 0);
    // Power = torque × angular speed (rpm × 2π / 60). Inherits the N·m assumption (A-LMU-053).
    w.float(
        ch::ENGINE_POWER,
        t.engine_torque * t.engine_rpm * TAU / SECONDS_PER_MINUTE,
    );

    let (reading, latched) = st.det.turbo.read(t.turbo_boost_pressure);
    if let Some(unit) = latched.filter(|u| *u == crate::detect::TurboUnit::Kilopascal) {
        st.raise(
            DiagnosticNotice::new(
                NoticeCode::TurboBoostKpa,
                Severity::Notice,
                Some(ch::TURBO_BOOST),
                format!("turbo boost reads in {unit:?}; converted to Pa (A-LMU-027)"),
            )
            .with_raw(t.turbo_boost_pressure),
        );
    }
    match reading {
        TurboReading::NotReported => w.quality(ch::TURBO_BOOST, Quality::NotSupported),
        TurboReading::Pascal(pa) => {
            let speed = convention::speed_mps(t.local_vel);
            fill_checked(st, w, (ch::TURBO_BOOST, t.turbo_boost_pressure, pa), speed);
        }
        TurboReading::Invalid => w.quality(ch::TURBO_BOOST, Quality::Invalid),
    }

    w.float(ch::FUEL, t.fuel);
    w.float(ch::FUEL_CAPACITY, t.fuel_capacity);
    let fuel_frac =
        (t.fuel_capacity.is_finite() && t.fuel_capacity > 0.0).then(|| t.fuel / t.fuel_capacity);
    w.opt_float(ch::FUEL_FRACTION, fuel_frac);
    w.int(ch::PIT_SCHEDULED_STOPS, i64::from(t.scheduled_stops));
}

// ---------------------------------------------------------------------------------------------------------
// Dynamics (§9, §18)
// ---------------------------------------------------------------------------------------------------------

fn map_dynamics(t: &RawTelemetry, w: &mut FrameWriter<'_>) {
    let vel = t.local_vel;
    let acc = t.local_accel;
    let rot = t.local_rot;
    let rot_acc = t.local_rot_accel;
    w.float(ch::LOCAL_VEL_X, vel.x);
    w.float(ch::LOCAL_VEL_Y, vel.y);
    w.float(ch::LOCAL_VEL_Z, vel.z);
    w.float(ch::LOCAL_ACCEL_X, acc.x);
    w.float(ch::LOCAL_ACCEL_Y, acc.y);
    w.float(ch::LOCAL_ACCEL_Z, acc.z);
    w.float(ch::LOCAL_ROT_X, rot.x);
    w.float(ch::LOCAL_ROT_Y, rot.y);
    w.float(ch::LOCAL_ROT_Z, rot.z);
    w.float(ch::LOCAL_ROT_ACCEL_X, rot_acc.x);
    w.float(ch::LOCAL_ROT_ACCEL_Y, rot_acc.y);
    w.float(ch::LOCAL_ROT_ACCEL_Z, rot_acc.z);
    // Orientation rows: 9 consecutive ids, row-major (r0.x, r0.y, r0.z, r1.x, …).
    for (r, row) in t.ori.iter().enumerate() {
        for (c, value) in [row.x, row.y, row.z].into_iter().enumerate() {
            let offset = (r * 3 + c) as u16;
            w.float(ChannelId(ch::ORIENTATION_R0_X.0 + offset), value);
        }
    }

    w.float(ch::SPEED, convention::speed_mps(vel));
    w.float(ch::SPEED_FORWARD, convention::forward_speed_mps(vel));
    let g_lat = convention::g_lat(acc);
    let g_long = convention::g_long(acc);
    w.float(ch::G_LAT, g_lat);
    w.float(ch::G_LONG, g_long);
    w.float(ch::G_VERT, convention::g_vert(acc));
    w.float(ch::G_COMBINED, convention::g_combined(g_lat, g_long));
    w.float(ch::YAW_RATE, convention::yaw_rate_radps(rot));
    w.float(ch::PITCH_RATE, convention::pitch_rate_radps(rot));
    w.float(ch::ROLL_RATE, convention::roll_rate_radps(rot));
    w.opt_float(ch::HEADING, convention::heading_rad(&t.ori));

    w.float(ch::POS_X, t.pos.x);
    w.float(ch::POS_Y, t.pos.y);
    w.float(ch::POS_Z, t.pos.z);
    // Track map (A-LMU-025): top view of a left-handed y-up world, screen right = +x, screen up = +z.
    w.float(ch::MAP_X, t.pos.x);
    w.float(ch::MAP_Y, t.pos.z);
}

// ---------------------------------------------------------------------------------------------------------
// Chassis, aero, suspension, wheels (§10)
// ---------------------------------------------------------------------------------------------------------

fn map_chassis(t: &RawTelemetry, st: &mut MapState<'_>, w: &mut FrameWriter<'_>) {
    let speed = convention::speed_mps(t.local_vel);
    for (id, raw) in [
        (ch::RIDE_HEIGHT_FRONT, t.front_ride_height),
        (ch::RIDE_HEIGHT_REAR, t.rear_ride_height),
        (ch::FRONT_WING_HEIGHT, t.front_wing_height),
        (ch::DRAG, t.drag),
        (ch::DOWNFORCE_FRONT, t.front_downforce),
        (ch::DOWNFORCE_REAR, t.rear_downforce),
    ] {
        fill_checked(st, w, (id, raw, raw), speed);
    }
    w.float(ch::THIRD_SPRING_FRONT, t.front3rd_deflection);
    w.float(ch::THIRD_SPRING_REAR, t.rear3rd_deflection);
    for wheel in Wheel::ALL {
        let raw = &t.wheels[wheel.index()];
        let id = |base| wheel_id(base, wheel);
        w.float(id(ch::SUSP_POS_FL), raw.suspension_deflection);
        w.float(id(ch::RIDE_HEIGHT_FL), raw.ride_height);
        w.float(id(ch::PUSHROD_FORCE_FL), raw.susp_force);
        w.boolean(id(ch::WHEEL_DETACHED_FL), raw.detached);
        if raw.detached {
            // A-LMU-061: the values of a wheel that came off are frozen or meaningless.
            for base in DETACHED_WHEEL_CHASSIS_CHANNELS {
                w.quality(id(base), Quality::Unavailable);
            }
            continue;
        }
        w.float(id(ch::CAMBER_FL), raw.camber);
        w.float(id(ch::TOE_FL), raw.toe);
        w.float(id(ch::WHEEL_ROT_FL), raw.rotation);
        w.float(id(ch::WHEEL_Y_LOCATION_FL), raw.wheel_y_location);
    }
}

/// Per-wheel chassis channels (FL ids) of the wheel itself, withheld while it is detached (A-LMU-061).
const DETACHED_WHEEL_CHASSIS_CHANNELS: [ChannelId; 4] = [
    ch::CAMBER_FL,
    ch::TOE_FL,
    ch::WHEEL_ROT_FL,
    ch::WHEEL_Y_LOCATION_FL,
];

/// Per-wheel tyre channels (FL ids) withheld while the wheel is detached (A-LMU-061). The flat flag and the
/// axle compound names stay published.
const DETACHED_WHEEL_TYRE_CHANNELS: [ChannelId; 29] = [
    ch::TYRE_PRESSURE_FL,
    ch::TYRE_TEMP_LEFT_FL,
    ch::TYRE_TEMP_MIDDLE_FL,
    ch::TYRE_TEMP_RIGHT_FL,
    ch::TYRE_LAYER_TEMP_LEFT_FL,
    ch::TYRE_LAYER_TEMP_MIDDLE_FL,
    ch::TYRE_LAYER_TEMP_RIGHT_FL,
    ch::TYRE_TEMP_INNER_FL,
    ch::TYRE_TEMP_OUTER_FL,
    ch::TYRE_TEMP_AVG_FL,
    ch::TYRE_TEMP_SPREAD_FL,
    ch::TYRE_CARCASS_TEMP_FL,
    ch::TYRE_OPTIMAL_TEMP_FL,
    ch::TYRE_WEAR_FL,
    ch::TYRE_SLIDING_FL,
    ch::TYRE_LOAD_FL,
    ch::TYRE_LAT_FORCE_FL,
    ch::TYRE_LONG_FORCE_FL,
    ch::TYRE_LAT_PATCH_VEL_FL,
    ch::TYRE_LONG_PATCH_VEL_FL,
    ch::TYRE_LAT_GROUND_VEL_FL,
    ch::TYRE_LONG_GROUND_VEL_FL,
    ch::TYRE_DEFLECTION_FL,
    ch::TYRE_RADIUS_FL,
    ch::WHEEL_SPEED_FL,
    ch::TYRE_COMPOUND_INDEX_FL,
    ch::TYRE_COMPOUND_TYPE_FL,
    ch::SURFACE_TYPE_FL,
    ch::TERRAIN_NAME_FL,
];

/// A Kelvin reading in °C; exactly 0 K (physically impossible) means "not initialised" (A-LMU-062).
fn initialised_kelvin(raw: f64) -> Option<f64> {
    (raw != 0.0).then(|| convert::kelvin_to_celsius(raw))
}

/// `Some` → the value; `None` → explicitly `Unavailable` (not left at a default from an earlier frame).
fn float_or_unavailable(w: &mut FrameWriter<'_>, id: ChannelId, v: Option<f64>) {
    match v {
        Some(v) => w.float(id, v),
        None => w.quality(id, Quality::Unavailable),
    }
}

// ---------------------------------------------------------------------------------------------------------
// Tyres (§11)
// ---------------------------------------------------------------------------------------------------------

fn map_tyres(
    t: &RawTelemetry,
    caps: VehicleCapabilities,
    st: &mut MapState<'_>,
    w: &mut FrameWriter<'_>,
) {
    w.text(ch::TYRE_COMPOUND_FRONT, t.front_tire_compound_name.as_str());
    w.text(ch::TYRE_COMPOUND_REAR, t.rear_tire_compound_name.as_str());
    w.int(
        ch::TYRE_COMPOUND_INDEX_FRONT,
        i64::from(t.front_tire_compound_index),
    );
    w.int(
        ch::TYRE_COMPOUND_INDEX_REAR,
        i64::from(t.rear_tire_compound_index),
    );
    for wheel in Wheel::ALL {
        let raw = &t.wheels[wheel.index()];
        let id = |base| wheel_id(base, wheel);
        w.boolean(id(ch::TYRE_FLAT_FL), raw.flat);
        if raw.detached {
            // A-LMU-061: a detached wheel's tyre values are frozen or meaningless.
            for base in DETACHED_WHEEL_TYRE_CHANNELS {
                w.quality(id(base), Quality::Unavailable);
            }
            continue;
        }

        // All three surface temperatures exactly 0 K: the tyre model is not initialised (garage/reset
        // transition, A-LMU-062), so the 0 kPa read in that state is not a measurement either. A flat tyre's
        // 0 kPa with real temperatures is published.
        let initialised = raw.temperature.iter().any(|&k| k != 0.0);
        if initialised {
            w.float(id(ch::TYRE_PRESSURE_FL), convert::kpa_to_pa(raw.pressure));
        } else {
            w.quality(id(ch::TYRE_PRESSURE_FL), Quality::Unavailable);
        }

        // Surface and inner-layer temperatures are Kelvin (A-LMU-049, verified at Gate 1), car-frame L/C/R.
        let surface = raw.temperature.map(initialised_kelvin);
        let layer = raw.tire_inner_layer_temperature.map(initialised_kelvin);
        let lcr_ids = [
            (ch::TYRE_TEMP_LEFT_FL, ch::TYRE_LAYER_TEMP_LEFT_FL),
            (ch::TYRE_TEMP_MIDDLE_FL, ch::TYRE_LAYER_TEMP_MIDDLE_FL),
            (ch::TYRE_TEMP_RIGHT_FL, ch::TYRE_LAYER_TEMP_RIGHT_FL),
        ];
        for (i, (surface_id, layer_id)) in lcr_ids.into_iter().enumerate() {
            float_or_unavailable(w, id(surface_id), surface[i]);
            float_or_unavailable(w, id(layer_id), layer[i]);
        }
        // Inner/outer (the side mapping lives in one function, A-LMU-016), average and spread need all three
        // surface readings.
        if let [Some(left), Some(centre), Some(right)] = surface {
            let surface = [left, centre, right];
            let edges = lcr_to_edges(wheel, surface);
            w.float(id(ch::TYRE_TEMP_INNER_FL), edges.inner);
            w.float(id(ch::TYRE_TEMP_OUTER_FL), edges.outer);
            w.float(id(ch::TYRE_TEMP_AVG_FL), surface.iter().sum::<f64>() / 3.0);
            w.float(id(ch::TYRE_TEMP_SPREAD_FL), edges.inner - edges.outer);
        } else {
            for base in [
                ch::TYRE_TEMP_INNER_FL,
                ch::TYRE_TEMP_OUTER_FL,
                ch::TYRE_TEMP_AVG_FL,
                ch::TYRE_TEMP_SPREAD_FL,
            ] {
                w.quality(id(base), Quality::Unavailable);
            }
        }
        float_or_unavailable(
            w,
            id(ch::TYRE_CARCASS_TEMP_FL),
            initialised_kelvin(raw.tire_carcass_temperature),
        );

        // mOptimalTemp: no header unit, °C CLAIMED; > 200 latches Kelvin (A-LMU-038).
        let optimal = f64::from(raw.optimal_temp);
        let latch = &mut st.det.optimal_temp[wheel.index()];
        if latch.observe(optimal) {
            st.raise(kelvin_notice(id(ch::TYRE_OPTIMAL_TEMP_FL), optimal));
        }
        w.float(
            id(ch::TYRE_OPTIMAL_TEMP_FL),
            st.det.optimal_temp[wheel.index()].to_celsius(optimal),
        );

        // mWear: 0 is ambiguous (not simulated, or a new tyre if 0 = unworn; direction UNVERIFIED,
        // A-LMU-040). Published only once a non-zero value proved the car simulates wear.
        match caps.supports_tyre_wear {
            Some(true) => w.float(id(ch::TYRE_WEAR_FL), raw.wear),
            Some(false) => w.quality(id(ch::TYRE_WEAR_FL), Quality::NotSupported),
            None => w.quality(id(ch::TYRE_WEAR_FL), Quality::Unavailable),
        }
        w.float(id(ch::TYRE_SLIDING_FL), raw.grip_fract);
        w.float(id(ch::TYRE_LOAD_FL), raw.tire_load);
        w.float(id(ch::TYRE_LAT_FORCE_FL), raw.lateral_force);
        w.float(id(ch::TYRE_LONG_FORCE_FL), raw.longitudinal_force);
        w.float(id(ch::TYRE_LAT_PATCH_VEL_FL), raw.lateral_patch_vel);
        w.float(id(ch::TYRE_LONG_PATCH_VEL_FL), raw.longitudinal_patch_vel);
        w.float(id(ch::TYRE_LAT_GROUND_VEL_FL), raw.lateral_ground_vel);
        w.float(id(ch::TYRE_LONG_GROUND_VEL_FL), raw.longitudinal_ground_vel);
        w.float(id(ch::TYRE_DEFLECTION_FL), raw.vertical_tire_deflection);
        // Static radius: u8 centimetres (1 cm resolution); 0 means "not provided".
        if raw.static_undeflected_radius == 0 {
            w.none(id(ch::TYRE_RADIUS_FL));
            w.none(id(ch::WHEEL_SPEED_FL));
        } else {
            let radius_m = convert::cm_to_m(f64::from(raw.static_undeflected_radius));
            w.float(id(ch::TYRE_RADIUS_FL), radius_m);
            // ESTIMATED: |rotation| × static radius (sign of mRotation UNVERIFIED, A-LMU-039).
            w.float(id(ch::WHEEL_SPEED_FL), raw.rotation.abs() * radius_m);
        }
        w.int(
            id(ch::TYRE_COMPOUND_INDEX_FL),
            i64::from(raw.compound_index),
        );
        w.int(id(ch::TYRE_COMPOUND_TYPE_FL), i64::from(raw.compound_type));
        w.int(id(ch::SURFACE_TYPE_FL), i64::from(raw.surface_type));
        w.text(id(ch::TERRAIN_NAME_FL), raw.terrain_name.as_str());
    }
}

fn kelvin_notice(channel: ChannelId, raw: f64) -> DiagnosticNotice {
    DiagnosticNotice::new(
        NoticeCode::TemperatureKelvinDetected,
        Severity::Notice,
        Some(channel),
        "temperature above 200 without a header unit: treated as Kelvin for the session (A-LMU-038)",
    )
    .with_raw(raw)
}

// ---------------------------------------------------------------------------------------------------------
// Brakes (§12)
// ---------------------------------------------------------------------------------------------------------

fn map_brakes(input: &MapInput<'_>, st: &mut MapState<'_>, w: &mut FrameWriter<'_>) {
    let t = input.telemetry;
    let et = Some(t.elapsed_time);
    let temps = t.wheels.each_ref().map(|wh| wh.brake_temp);
    let speed = convention::speed_mps(t.local_vel);
    let obs = BrakeObservation {
        raw: temps,
        et_s: t.elapsed_time,
        speed_mps: speed.is_finite().then_some(speed),
        in_realtime: input.info.in_realtime,
        ambient_c: finite(input.info.ambient_temp),
    };
    st.det
        .brake_unit
        .set_verified_unit(verified_brake_temp_unit(input.generic.game_version));
    let transition = st.det.brake_unit.observe(&obs);
    let decision = st.det.brake_unit.decision();
    match transition {
        BrakeUnitTransition::Decided(unit) => st.raise(
            DiagnosticNotice::new(
                NoticeCode::BrakeTempUnitDecided,
                Severity::Info,
                Some(ch::BRAKE_TEMP_FL),
                format!(
                    "brake temperature unit decided from evidence: {unit:?} ({} K / {} °C votes)",
                    decision.kelvin_votes, decision.celsius_votes
                ),
            )
            .at(et),
        ),
        BrakeUnitTransition::BecameConflicting => st.raise(
            DiagnosticNotice::new(
                NoticeCode::BrakeTempUnitConflict,
                Severity::Warning,
                Some(ch::BRAKE_TEMP_FL),
                "conflicting brake temperature unit evidence (or evidence against the unit verified for this \
                 LMU version): brake temperatures withheld (UNAVAILABLE) for the session (A-LMU-015)",
            )
            .at(et),
        ),
        BrakeUnitTransition::None => {}
    }
    let decided = st.det.brake_unit.is_decided();
    if !decided {
        st.raise(
            DiagnosticNotice::new(
                NoticeCode::BrakeTempUnitUnverified,
                Severity::Notice,
                Some(ch::BRAKE_TEMP_FL),
                "brake temperature unit (K or °C) not yet decided by evidence: brake temperatures withheld \
                 (UNAVAILABLE), raw front-left value attached (A-LMU-015)",
            )
            .with_raw(temps[Wheel::FrontLeft.index()])
            .at(et),
        );
    }
    for wheel in Wheel::ALL {
        let id = wheel_id(ch::BRAKE_TEMP_FL, wheel);
        if decided {
            w.float(id, st.det.brake_unit.to_celsius(temps[wheel.index()]));
        } else {
            // An unverified conversion is a possible silent 273 °C error: never published as VALID.
            w.quality(id, Quality::Unavailable);
        }
    }

    // mBrakePressure is a 0..1 fraction (A-LMU-017); a value above 1.5 means LMU switched to kPa.
    let pressures = t.wheels.each_ref().map(|wh| wh.brake_pressure);
    if let Some(trigger) = st.det.brake_pressure.observe(pressures) {
        st.raise(
            DiagnosticNotice::new(
                NoticeCode::BrakePressureUnitChanged,
                Severity::Warning,
                Some(ch::BRAKE_PRESSURE_FL),
                "mBrakePressure above 1.5: LMU no longer reports a 0..1 fraction (kPa?); brake pressure \
                 mapping stopped for this session (A-LMU-017)",
            )
            .with_raw(trigger)
            .at(et),
        );
    }
    for wheel in Wheel::ALL {
        let id = wheel_id(ch::BRAKE_PRESSURE_FL, wheel);
        if st.det.brake_pressure.unit_changed() {
            w.quality(id, Quality::Invalid);
        } else {
            w.float(id, pressures[wheel.index()]);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Hybrid and energy (§7)
// ---------------------------------------------------------------------------------------------------------

/// Quality for a hybrid channel the current sample does not provide.
fn hybrid_absent(caps: VehicleCapabilities) -> Quality {
    if caps.supports_hybrid == Some(false) {
        Quality::NotSupported
    } else {
        Quality::Unavailable
    }
}

/// A fraction channel through its scale detector; announces scale changes once. Withheld (`UNAVAILABLE`)
/// while the scale is undecided, `INVALID` once unrecognized.
fn scaled_fraction(
    st: &mut MapState<'_>,
    w: &mut FrameWriter<'_>,
    id: ChannelId,
    (raw, lap): (f64, i32),
    pick: fn(&mut SessionDetectors) -> &mut FractionScaleDetector,
) {
    if !raw.is_finite() {
        w.quality(id, Quality::Invalid);
        return;
    }
    if let Some(scale) = pick(st.det).observe(raw, lap) {
        let (code, severity, message) = match scale {
            FractionScale::Percent => (
                NoticeCode::FractionScalePercent,
                Severity::Notice,
                "value above 1.5: scale detected as percent, divided by 100 for the session",
            ),
            FractionScale::Fraction => (
                NoticeCode::FractionScaleFraction,
                Severity::Info,
                "values stayed in 0..1.5 and changed over two laps: scale confirmed as 0..1",
            ),
            FractionScale::Conflicting => (
                NoticeCode::FractionScaleConflict,
                Severity::Warning,
                "value contradicts the scale verified for this LMU version (LMU changed it?): withheld \
                 (UNAVAILABLE) for the session (A-LMU-013/014)",
            ),
            FractionScale::Unrecognized | FractionScale::Undecided => (
                NoticeCode::FractionScaleUnrecognized,
                Severity::Warning,
                "value outside the 0..1 and 0..100 scales (or above 1.5 after 0..1 was confirmed): \
                 published INVALID for the session (unit unknown, never converted to an invented unit)",
            ),
        };
        st.raise(DiagnosticNotice::new(code, severity, Some(id), message).with_raw(raw));
    }
    let scale = pick(st.det).scale();
    match (scale, pick(st.det).to_fraction(raw)) {
        (_, Some(frac)) => w.float(id, frac),
        (FractionScale::Undecided, None) => {
            st.raise(
                DiagnosticNotice::new(
                    NoticeCode::FractionScaleUndecided,
                    Severity::Info,
                    Some(id),
                    "value in 0..1.5 fits both the 0..1 and the percent scale: withheld until a value above \
                     1.5 or two laps of changing values decide it (A-LMU-013/014)",
                )
                .with_raw(raw),
            );
            w.quality(id, Quality::Unavailable);
        }
        // Withheld, exactly like a brake-unit conflict: the scale is unknown, the value may be fine.
        (FractionScale::Conflicting, None) => w.quality(id, Quality::Unavailable),
        (_, None) => w.quality(id, Quality::Invalid),
    }
}

fn map_energy(input: &MapInput<'_>, st: &mut MapState<'_>, w: &mut FrameWriter<'_>) {
    let t = input.telemetry;
    let caps = input.caps;
    let state = t.electric_boost_motor_state;
    w.int(ch::HYBRID_MOTOR_STATE, i64::from(state));
    w.int(ch::LIFT_AND_COAST, i64::from(t.lift_and_coast_progress));

    // Hybrid channels exist when the car is known to be hybrid, or (undecided) the motor reports a state.
    let hybrid_present = match caps.supports_hybrid {
        Some(known) => known,
        None => state != 0,
    };
    if hybrid_present {
        w.float(ch::HYBRID_MOTOR_TORQUE, t.electric_boost_motor_torque);
        w.float(ch::HYBRID_MOTOR_RPM, t.electric_boost_motor_rpm);
        let motor = t.electric_boost_motor_temperature;
        if st.det.motor_temp.observe(motor) {
            st.raise(kelvin_notice(ch::HYBRID_MOTOR_TEMP, motor));
        }
        w.float(ch::HYBRID_MOTOR_TEMP, st.det.motor_temp.to_celsius(motor));
        // "0 if there is no cooler" (header): exactly 0 is the sentinel.
        let water = t.electric_boost_water_temperature;
        if water == 0.0 {
            w.none(ch::HYBRID_WATER_TEMP);
        } else {
            if st.det.motor_water_temp.observe(water) {
                st.raise(kelvin_notice(ch::HYBRID_WATER_TEMP, water));
            }
            w.float(
                ch::HYBRID_WATER_TEMP,
                st.det.motor_water_temp.to_celsius(water),
            );
        }
        w.float(ch::REGEN_POWER, convert::kw_to_w(f64::from(t.regen)));
        w.float(ch::BATTERY_CHARGE, t.battery_charge_fraction);
        let soc = (f64::from(t.soc), t.lap_number);
        scaled_fraction(st, w, ch::BATTERY_SOC, soc, |d| &mut d.soc);
    } else {
        let q = hybrid_absent(caps);
        crate::capability::HYBRID_GATED
            .into_iter()
            .for_each(|id| w.quality(id, q));
    }

    // Virtual energy: only cars that ever report a non-zero value use it (data-driven, A-LMU-012).
    let ve = f64::from(t.virtual_energy);
    match caps.supports_virtual_energy {
        _ if !ve.is_finite() => w.quality(ch::VIRTUAL_ENERGY, Quality::Invalid),
        Some(false) => w.quality(ch::VIRTUAL_ENERGY, Quality::NotSupported),
        None if ve == 0.0 => w.quality(ch::VIRTUAL_ENERGY, Quality::Unavailable),
        Some(true) | None => {
            // 0..1 is verified for some LMU versions (A-LMU-013): applied from the first value there.
            st.det
                .virtual_energy
                .set_verified_scale(verified_virtual_energy_scale(input.generic.game_version));
            let raw = (ve, t.lap_number);
            scaled_fraction(st, w, ch::VIRTUAL_ENERGY, raw, |d| &mut d.virtual_energy);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Timing (§3) and track position (§14)
// ---------------------------------------------------------------------------------------------------------

/// `mNumVehicles` when the player is alone in the session.
const SOLO_SESSION_VEHICLES: i32 = 1;

fn map_timing(t: &RawTelemetry, info: &RawScoringInfo, w: &mut FrameWriter<'_>) {
    w.float(ch::ELAPSED_TIME, t.elapsed_time);
    w.float(ch::UPDATE_DELTA_TIME, t.delta_time);
    w.int(ch::LAP_NUMBER, i64::from(t.lap_number));
    let lap_start = lap_start_et(t.lap_start_et);
    w.opt_float(ch::LAP_START_TIME, lap_start);
    w.opt_float(ch::LAP_TIME, lap_start.map(|s| t.elapsed_time - s));
    w.boolean(ch::LAP_INVALID, t.lap_invalidated);
    w.float(ch::DELTA_BEST, t.delta_best);
    let gaps = [
        (ch::GAP_CAR_AHEAD, t.time_gap_car_ahead),
        (ch::GAP_CAR_BEHIND, t.time_gap_car_behind),
        (ch::GAP_PLACE_AHEAD, t.time_gap_place_ahead),
        (ch::GAP_PLACE_BEHIND, t.time_gap_place_behind),
    ];
    // Alone in the session there is no car to measure a gap to: LMU's 0 is its "no car" value (Gate 1, solo
    // practice: exactly 0 all session, A-LMU-066), not a 0 s gap. Left at "no data" (the map's "or NONE").
    let solo = info.num_vehicles == SOLO_SESSION_VEHICLES;
    for (id, raw) in gaps {
        if solo {
            w.none(id);
        } else {
            w.float(id, f64::from(raw));
        }
    }
}

/// Decodes telemetry `mCurrentSector`: (zero-based sector index if valid, in pit lane) (A-LMU-026).
pub fn decode_current_sector(raw: i32) -> (Option<u8>, bool) {
    // Low 31 bits: zero-based sector index (0 = S1); bit 31: pit lane. The reader splits them.
    let (bits, in_pit_lane) = lmu_reader::split_current_sector(raw);
    let index = if bits <= MAX_SECTOR_INDEX {
        u8::try_from(bits).ok()
    } else {
        None
    };
    (index, in_pit_lane)
}

/// Decodes scoring `mSector` (0 = S3, 1 = S1, 2 = S2) to a zero-based index (0 = S1).
pub fn decode_scoring_sector(raw: i8) -> Option<u8> {
    (0..SECTORS_PER_LAP)
        .contains(&raw)
        .then(|| (raw + SCORING_SECTOR_OFFSET) % SECTORS_PER_LAP)
        .and_then(|i| u8::try_from(i).ok())
}

fn map_track_position(t: &RawTelemetry, w: &mut FrameWriter<'_>) {
    w.int(ch::SECTOR_RAW, i64::from(t.current_sector));
    let (sector, in_pit_lane) = decode_current_sector(t.current_sector);
    match sector {
        Some(s) => w.int(ch::SECTOR, i64::from(s)),
        None => w.quality(ch::SECTOR, Quality::Invalid),
    }
    w.boolean(ch::IN_PIT_LANE, in_pit_lane);
    w.int(ch::TRACK_LIMITS_STEPS, i64::from(t.track_limits_steps));
}

// ---------------------------------------------------------------------------------------------------------
// Session, weather and flags (§1, §15, §16)
// ---------------------------------------------------------------------------------------------------------

fn map_session(info: &RawScoringInfo, st: &mut MapState<'_>, w: &mut FrameWriter<'_>) {
    w.text(ch::TRACK_NAME, info.track_name.as_str());
    w.int(ch::SESSION_TYPE, i64::from(info.session));
    w.float(ch::SESSION_TIME, info.current_et);
    w.float(ch::SESSION_END_TIME, info.end_et);
    w.opt_float(
        ch::SESSION_TIME_REMAINING,
        non_negative(f64::from(info.session_time_remaining)),
    );
    w.opt_float(
        ch::SESSION_TIME_REMAINING_CALC,
        session_time_remaining(info.end_et, info.current_et),
    );
    w.opt_int(
        ch::SESSION_MAX_LAPS,
        lmu_max_laps(info.max_laps).map(i64::from),
    );
    w.int(ch::NUM_VEHICLES, i64::from(info.num_vehicles));
    w.int(ch::GAME_PHASE, i64::from(info.game_phase));
    w.boolean(ch::IN_REALTIME, info.in_realtime);
    w.int(ch::START_LIGHT, i64::from(info.start_light));
    w.int(ch::NUM_RED_LIGHTS, i64::from(info.num_red_lights));
    w.float(ch::START_TIME_OF_DAY, f64::from(info.start_et));
    let tod = st.det.time_of_day.read(f64::from(info.time_of_day));
    if !st.det.time_of_day.is_decided() {
        st.raise(
            DiagnosticNotice::new(
                NoticeCode::TimeOfDayScaleUndecided,
                Severity::Info,
                Some(ch::TIME_OF_DAY),
                "mTimeOfDay not above 24 yet: hours vs seconds undecided, withheld (A-LMU-036)",
            )
            .with_raw(f64::from(info.time_of_day)),
        );
    }
    w.opt_float(ch::TIME_OF_DAY, tod);
    w.boolean(ch::FIXED_SETUP, info.is_fixed_setup);
    w.int(ch::GAME_MODE, i64::from(info.game_mode));
    w.int(ch::MAX_PLAYERS, i64::from(info.max_players));
    w.int(
        ch::TRACK_LIMITS_STEPS_PER_PENALTY,
        i64::from(info.track_limits_steps_per_penalty),
    );
    w.int(
        ch::TRACK_LIMITS_STEPS_PER_POINT,
        i64::from(info.track_limits_steps_per_point),
    );
    w.float(ch::TRACK_LENGTH, info.lap_dist);

    w.float(ch::AIR_TEMP, info.ambient_temp);
    w.float(ch::TRACK_TEMP, info.track_temp);
    w.float(ch::RAIN, info.raining);
    w.float(ch::DARK_CLOUD, info.dark_cloud);
    w.int(ch::CLOUD_COVERAGE, i64::from(info.cloud_coverage));
    w.float(ch::TRACK_WETNESS_MIN, info.min_path_wetness);
    w.float(ch::TRACK_WETNESS_MAX, info.max_path_wetness);
    w.float(ch::TRACK_WETNESS, info.avg_path_wetness);
    w.float(ch::WIND_X, info.wind.x);
    w.float(ch::WIND_Y, info.wind.y);
    w.float(ch::WIND_Z, info.wind.z);
    w.int(ch::TRACK_GRIP_LEVEL, i64::from(info.track_grip_level));

    w.int(ch::YELLOW_FLAG_STATE, i64::from(info.yellow_flag_state));
    let sector_flag_ids = [ch::SECTOR_FLAG_0, ch::SECTOR_FLAG_1, ch::SECTOR_FLAG_2];
    for (id, &flag) in sector_flag_ids.into_iter().zip(info.sector_flag.iter()) {
        w.int(id, i64::from(flag));
    }
    let (fcy_lo, fcy_hi) = YELLOW_STATE_FCY;
    let fcy =
        info.game_phase == GAME_PHASE_FCY || (fcy_lo..=fcy_hi).contains(&info.yellow_flag_state);
    w.boolean(ch::FULL_COURSE_YELLOW, fcy);
}

/// `mEndET − mCurrentET`, clamped at 0 once the session has run out. `None` when `mEndET` is not a real
/// end time (not finite or ≤ 0: e.g. the UNVERIFIED lap-race sentinel), never a fabricated "0 remaining".
pub fn session_time_remaining(end_et: f64, current_et: f64) -> Option<f64> {
    if !(end_et.is_finite() && end_et > 0.0) {
        return None;
    }
    let remaining = end_et - current_et;
    remaining.is_finite().then(|| remaining.max(0.0))
}

// ---------------------------------------------------------------------------------------------------------
// Player scoring row (§3, §14, §15)
// ---------------------------------------------------------------------------------------------------------

/// Player scoring channels (left at "no data" when the row is unknown).
const PLAYER_SCORING_CHANNELS: &[ChannelId] = &[
    ch::BEST_LAP_TIME,
    ch::LAST_LAP_TIME,
    ch::BEST_SECTOR1,
    ch::BEST_SECTOR12,
    ch::LAST_SECTOR1,
    ch::LAST_SECTOR12,
    ch::CUR_SECTOR1,
    ch::CUR_SECTOR12,
    ch::BEST_LAP_SECTOR1,
    ch::BEST_LAP_SECTOR12,
    ch::TIME_INTO_LAP,
    ch::ESTIMATED_LAP_TIME,
    ch::LAP_START_TIME_SCORING,
    ch::LAST_SECTOR2,
    ch::LAST_SECTOR3,
    ch::BEST_SECTOR2,
    ch::POSITION,
    ch::CLASS_POSITION,
    ch::LAPS_COMPLETED,
    ch::FINISH_STATUS,
    ch::TIME_BEHIND_NEXT,
    ch::LAPS_BEHIND_NEXT,
    ch::TIME_BEHIND_LEADER,
    ch::LAPS_BEHIND_LEADER,
    ch::QUALIFYING_POSITION,
    ch::CONTROL,
    ch::SERVER_SCORED,
    ch::COUNT_LAP_FLAG,
    ch::PENALTIES,
    ch::DISTANCE,
    ch::LAP_DISTANCE,
    ch::LAP_PROGRESS,
    ch::SECTOR_SCORING,
    ch::SECTOR_SCORING_RAW,
    ch::PATH_LATERAL,
    ch::TRACK_EDGE,
    ch::CAR_FLAG,
    ch::UNDER_YELLOW,
    ch::INDIVIDUAL_PHASE,
    ch::BLUE_FLAG,
    ch::IN_PITS,
    ch::PIT_STATE,
    ch::IN_GARAGE,
    ch::PIT_LAP_DISTANCE,
    ch::PIT_STOPS,
    ch::VEHICLE_CLASS_NAME,
    ch::VEHICLE_FILENAME,
];

/// Lap progress (ESTIMATED): the last scoring lap distance, extrapolated to the telemetry ET with the
/// forward speed, as a fraction of the track length in [0, 1). `None` when the track length is unknown or
/// the scoring anchor is more than [`MAX_PROGRESS_EXTRAPOLATION_S`] away.
pub fn lap_progress(
    lap_dist_m: f64,
    track_length_m: f64,
    scoring_et_s: f64,
    telemetry_et_s: f64,
    forward_speed_mps: f64,
) -> Option<f64> {
    if !(track_length_m.is_finite() && track_length_m > 0.0 && lap_dist_m.is_finite()) {
        return None;
    }
    let dt = telemetry_et_s - scoring_et_s;
    if !(dt.is_finite() && dt.abs() <= MAX_PROGRESS_EXTRAPOLATION_S) {
        return None;
    }
    let anchor = (lap_dist_m / track_length_m).clamp(0.0, PROGRESS_MAX);
    let advance = if forward_speed_mps.is_finite() {
        forward_speed_mps * dt / track_length_m
    } else {
        0.0
    };
    let mut progress = anchor + advance;
    if progress >= 1.0 {
        // Crossed the line since the scoring update.
        progress -= 1.0;
    }
    Some(progress.clamp(0.0, PROGRESS_MAX))
}

fn map_player_scoring(input: &MapInput<'_>, w: &mut FrameWriter<'_>) {
    let Some(row) = input.player_row else {
        PLAYER_SCORING_CHANNELS.iter().for_each(|&id| w.none(id));
        return;
    };
    let entry: CompetitorEntry = crate::standings::competitor(row);
    w.opt_float(ch::BEST_LAP_TIME, entry.best_lap_s);
    w.opt_float(ch::LAST_LAP_TIME, entry.last_lap_s);
    w.opt_float(ch::BEST_SECTOR1, entry.best_sector1_s);
    w.opt_float(ch::BEST_SECTOR12, entry.best_sector12_s);
    w.opt_float(ch::LAST_SECTOR1, entry.last_sector1_s);
    w.opt_float(ch::LAST_SECTOR12, entry.last_sector12_s);
    w.opt_float(ch::CUR_SECTOR1, entry.cur_sector1_s);
    w.opt_float(ch::CUR_SECTOR12, entry.cur_sector12_s);
    w.opt_float(
        ch::BEST_LAP_SECTOR1,
        lmu_time(f64::from(row.best_lap_sector1)),
    );
    w.opt_float(
        ch::BEST_LAP_SECTOR12,
        lmu_time(f64::from(row.best_lap_sector2)),
    );
    w.opt_float(ch::TIME_INTO_LAP, entry.time_into_lap_s);
    w.opt_float(ch::ESTIMATED_LAP_TIME, entry.estimated_lap_s);
    w.opt_float(ch::LAP_START_TIME_SCORING, entry.lap_start_et_s);
    w.opt_float(ch::LAST_SECTOR2, entry.last_sector2_s());
    w.opt_float(ch::LAST_SECTOR3, entry.last_sector3_s());
    w.opt_float(ch::BEST_SECTOR2, entry.best_sector2_s());

    // mPlace 0 = not classified yet (loading): no data, as in the standings, never an out-of-range 0.
    w.opt_int(
        ch::POSITION,
        (row.place >= 1).then_some(i64::from(row.place)),
    );
    w.opt_int(ch::CLASS_POSITION, input.class_place.map(i64::from));
    w.int(ch::LAPS_COMPLETED, i64::from(row.total_laps));
    w.int(ch::FINISH_STATUS, i64::from(row.finish_status));
    w.opt_float(ch::TIME_BEHIND_NEXT, entry.time_behind_next_s);
    w.int(ch::LAPS_BEHIND_NEXT, i64::from(row.laps_behind_next));
    w.opt_float(ch::TIME_BEHIND_LEADER, entry.time_behind_leader_s);
    w.int(ch::LAPS_BEHIND_LEADER, i64::from(row.laps_behind_leader));
    w.opt_int(
        ch::QUALIFYING_POSITION,
        qualification(row.qualification).map(i64::from),
    );
    w.int(ch::CONTROL, i64::from(row.control));
    w.boolean(ch::SERVER_SCORED, row.server_scored != 0);
    w.int(ch::COUNT_LAP_FLAG, i64::from(row.count_lap_flag));
    w.int(ch::PENALTIES, i64::from(row.num_penalties));

    let track_length = input.info.lap_dist;
    w.float(ch::LAP_DISTANCE, row.lap_dist);
    let distance = (track_length.is_finite() && track_length > 0.0 && row.total_laps >= 0)
        .then(|| f64::from(row.total_laps) * track_length + row.lap_dist);
    w.opt_float(ch::DISTANCE, distance);
    let forward = convention::forward_speed_mps(input.telemetry.local_vel);
    w.opt_float(
        ch::LAP_PROGRESS,
        lap_progress(
            row.lap_dist,
            track_length,
            input.info.current_et,
            input.telemetry.elapsed_time,
            forward,
        ),
    );
    w.int(ch::SECTOR_SCORING_RAW, i64::from(row.sector));
    match decode_scoring_sector(row.sector) {
        Some(s) => w.int(ch::SECTOR_SCORING, i64::from(s)),
        None => w.quality(ch::SECTOR_SCORING, Quality::Invalid),
    }
    w.float(ch::PATH_LATERAL, row.path_lateral);
    w.float(ch::TRACK_EDGE, row.track_edge);

    w.int(ch::CAR_FLAG, i64::from(row.flag));
    w.boolean(ch::BLUE_FLAG, row.flag == CAR_FLAG_BLUE);
    w.boolean(ch::UNDER_YELLOW, row.under_yellow);
    w.int(ch::INDIVIDUAL_PHASE, i64::from(row.individual_phase));

    w.boolean(ch::IN_PITS, row.in_pits);
    w.int(ch::PIT_STATE, i64::from(row.pit_state));
    w.boolean(ch::IN_GARAGE, row.in_garage_stall);
    w.float(ch::PIT_LAP_DISTANCE, f64::from(row.pit_lap_dist));
    w.int(ch::PIT_STOPS, i64::from(row.num_pitstops));
    w.opt_text(ch::VEHICLE_CLASS_NAME, text(&row.vehicle_class));
    w.opt_text(ch::VEHICLE_FILENAME, text(&row.veh_filename));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_sector_and_pit_bit() {
        assert_eq!(decode_current_sector(0), (Some(0), false));
        assert_eq!(decode_current_sector(2), (Some(2), false));
        // 0x80000002: entering the pits from the third sector (header example).
        let pit = i32::from_le_bytes(0x8000_0002u32.to_le_bytes());
        assert_eq!(decode_current_sector(pit), (Some(2), true));
        assert_eq!(
            decode_current_sector(7),
            (None, false),
            "invalid sector index"
        );
    }

    #[test]
    fn scoring_sector_zero_is_sector_three() {
        assert_eq!(decode_scoring_sector(1), Some(0), "S1");
        assert_eq!(decode_scoring_sector(2), Some(1), "S2");
        assert_eq!(decode_scoring_sector(0), Some(2), "S3");
        assert_eq!(decode_scoring_sector(3), None);
        assert_eq!(decode_scoring_sector(-1), None);
    }

    #[test]
    fn lap_progress_is_anchored_and_extrapolated() {
        let p = lap_progress(1_000.0, 10_000.0, 100.0, 100.0, 50.0);
        assert!(p.is_some_and(|p| (p - 0.1).abs() < 1e-12));
        let p = lap_progress(1_000.0, 10_000.0, 100.0, 100.5, 50.0);
        assert!(p.is_some_and(|p| (p - 0.1025).abs() < 1e-12));
        // Crossing the line between scoring updates wraps to the new lap.
        let p = lap_progress(9_990.0, 10_000.0, 100.0, 100.4, 50.0);
        assert!(p.is_some_and(|p| (p - 0.001).abs() < 1e-9));
        assert_eq!(lap_progress(1_000.0, 0.0, 100.0, 100.0, 50.0), None);
        assert_eq!(
            lap_progress(1_000.0, 10_000.0, 100.0, 105.0, 50.0),
            None,
            "stale anchor"
        );
        let neg = lap_progress(-20.0, 10_000.0, 100.0, 100.0, 0.0);
        assert_eq!(neg, Some(0.0), "clamped, never negative");
    }
}

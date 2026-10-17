//! Writes the player's state into a [`TelemetryFrame`] the way the production pipeline would: measured
//! channels and the normalizer-derived ones (lmu-normalizer `DERIVED_HERE`), scoring-sourced channels from
//! the latest scoring snapshot (so they change at the scoring rate), and never a channel the registry marks
//! `NotProvided` or `Experimental`. race-engines' calculated channels are left `Unavailable` for the
//! engines under test to compute.

use std::f64::consts::PI;

use telemetry_model::ch;
use telemetry_model::channel::{Availability, ChannelId};
use telemetry_model::enums::{CountLapFlag, HybridMotorState, IgnitionStarter, YellowFlagState};
use telemetry_model::frame::{FrameError, TelemetryFrame};
use telemetry_model::registry;
use telemetry_model::units::convert;
use telemetry_model::value::{Sample, Value};

use crate::car::Car;
use crate::dynamics::{Dynamics, WHEELS};
use crate::error::SyntheticError;
use crate::session::ScoringSnapshot;
use crate::track::{Track, wrap_pi};
use crate::vehicle::{ChannelFill, VehicleProfile, rpm_to_radps};
use crate::weather::WeatherState;

/// Bit 31 of LMU `mCurrentSector`: set while in the pit lane (A-LMU-026).
const CURRENT_SECTOR_PIT_BIT: u32 = 0x8000_0000;
/// Raw scoring `mSector` of sector 3 (0 = S3, 1 = S1, 2 = S2).
const SCORING_SECTOR_RAW_S3: i8 = 0;
/// Wiper states (raw, scale unverified): off, slow, fast; rain thresholds.
const WIPER_SLOW_RAIN: f64 = 0.05;
const WIPER_FAST_RAIN: f64 = 0.5;
/// Headlights come on in heavy cloud or rain.
const HEADLIGHT_DARK_CLOUD: f64 = 0.8;
const HEADLIGHT_RAIN: f64 = 0.3;
/// Static settings written for the player car.
const MOTOR_MAP: u8 = 3;
const MOTOR_MAP_MAX: u8 = 8;
const ARB_SETTING: u8 = 5;
const ARB_SETTING_MAX: u8 = 10;
const BRAKE_MIGRATION: u8 = 2;
const BRAKE_MIGRATION_MAX: u8 = 5;
const TRACK_LIMITS_STEPS_PER_PENALTY: u8 = 4;
const TRACK_LIMITS_STEPS_PER_POINT: u8 = 1;
/// Terrain names reported under the wheels.
const TERRAIN_ROAD: &str = "ROAD";
const TERRAIN_GRASS: &str = "GRASS";
/// Number of dent locations.
const DENT_LOCATIONS: u16 = 8;
/// Seconds per day.
const SECONDS_PER_DAY: f64 = 86_400.0;
/// Margin keeping the heading strictly inside ±π after narrowing to f32 (f32 π rounds above f64 π).
const HEADING_F32_MARGIN_RAD: f64 = 1e-6;

/// Everything needed to write one frame.
#[derive(Debug, Clone, Copy)]
pub struct FrameContext<'a> {
    pub et_s: f64,
    pub dt_s: f64,
    pub track: &'a Track,
    pub vehicle: &'a VehicleProfile,
    pub car: &'a Car,
    pub dynamics: &'a Dynamics,
    pub scoring: &'a ScoringSnapshot,
    pub weather: &'a WeatherState,
    /// World position (x, y, z) of the player (m).
    pub position: (f64, f64, f64),
    /// Heading of the car (rad).
    pub heading_rad: f64,
    /// Lateral offset from the racing line, + = right (m).
    pub lateral_m: f64,
    pub delta_best_s: Option<f64>,
    /// Vehicles with telemetry (the whole field).
    pub vehicle_count: usize,
    /// Which channels the simulated game fills; unfilled ones are written UNAVAILABLE.
    pub channel_fill: ChannelFill,
}

/// Typed, availability-gated writes into a frame.
struct Writer<'a> {
    frame: &'a mut TelemetryFrame,
}

impl Writer<'_> {
    fn put(&mut self, id: ChannelId, value: Value) -> Result<(), SyntheticError> {
        let def = registry::by_id(id).ok_or(FrameError::UnknownChannel(id.0))?;
        if matches!(
            def.availability,
            Availability::NotProvided | Availability::Experimental
        ) {
            // Production never publishes these; neither does the synthetic source.
            return Ok(());
        }
        self.frame.set_valid(id, value)?;
        Ok(())
    }
    fn f32(&mut self, id: ChannelId, v: f64) -> Result<(), SyntheticError> {
        // Narrowing to the registry's wire type; generator values are far inside the f32 range.
        self.put(id, Value::F32(v as f32))
    }
    fn f64(&mut self, id: ChannelId, v: f64) -> Result<(), SyntheticError> {
        self.put(id, Value::F64(v))
    }
    fn opt_f32(&mut self, id: ChannelId, v: Option<f64>) -> Result<(), SyntheticError> {
        match v {
            Some(v) => self.f32(id, v),
            None => self.unavailable(id),
        }
    }
    fn opt_f64(&mut self, id: ChannelId, v: Option<f64>) -> Result<(), SyntheticError> {
        match v {
            Some(v) => self.f64(id, v),
            None => self.unavailable(id),
        }
    }
    fn i32(&mut self, id: ChannelId, v: i32) -> Result<(), SyntheticError> {
        self.put(id, Value::I32(v))
    }
    fn u8(&mut self, id: ChannelId, v: u8) -> Result<(), SyntheticError> {
        self.put(id, Value::U8(v))
    }
    fn i8(&mut self, id: ChannelId, v: i8) -> Result<(), SyntheticError> {
        self.put(id, Value::I8(v))
    }
    fn boolean(&mut self, id: ChannelId, v: bool) -> Result<(), SyntheticError> {
        self.put(id, Value::Bool(v))
    }
    fn text(&mut self, id: ChannelId, v: &str) -> Result<(), SyntheticError> {
        self.put(id, Value::Str(v.to_owned()))
    }
    fn opt_i32(&mut self, id: ChannelId, v: Option<i32>) -> Result<(), SyntheticError> {
        match v {
            Some(v) => self.i32(id, v),
            None => self.unavailable(id),
        }
    }
    fn opt_u8(&mut self, id: ChannelId, v: Option<u8>) -> Result<(), SyntheticError> {
        match v {
            Some(v) => self.u8(id, v),
            None => self.unavailable(id),
        }
    }
    fn opt_bool(&mut self, id: ChannelId, v: Option<bool>) -> Result<(), SyntheticError> {
        match v {
            Some(v) => self.boolean(id, v),
            None => self.unavailable(id),
        }
    }
    /// A setting gated by its maximum, like the normalizer's `gated_setting`: the maximum is always
    /// published (0 = the car has no such adjustable system) and the setting is NOT_SUPPORTED at max 0.
    fn gated_setting(
        &mut self,
        id: ChannelId,
        value: u8,
        max_id: ChannelId,
        max: u8,
    ) -> Result<(), SyntheticError> {
        self.u8(max_id, max)?;
        if max == 0 {
            self.not_supported(id)
        } else {
            self.u8(id, value)
        }
    }
    fn not_supported(&mut self, id: ChannelId) -> Result<(), SyntheticError> {
        self.frame.set(id, Sample::not_supported())?;
        Ok(())
    }
    fn unavailable(&mut self, id: ChannelId) -> Result<(), SyntheticError> {
        self.frame.set(id, Sample::unavailable())?;
        Ok(())
    }
}

/// Channel id of `wheel` (0..4) in a per-wheel group starting at `front_left`.
fn wheel(front_left: ChannelId, wheel: usize) -> ChannelId {
    // Per-wheel groups use a stride of one id (FL, FR, RL, RR); `wheel` < 4.
    ChannelId(front_left.0 + wheel as u16)
}

/// Writes every channel the synthetic source provides.
pub fn write_frame(
    frame: &mut TelemetryFrame,
    ctx: &FrameContext<'_>,
) -> Result<(), SyntheticError> {
    let mut w = Writer { frame };
    write_inputs(&mut w, ctx)?;
    write_vehicle(&mut w, ctx)?;
    write_powertrain(&mut w, ctx)?;
    write_tyres_and_brakes(&mut w, ctx)?;
    write_body(&mut w, ctx)?;
    write_energy(&mut w, ctx)?;
    write_timing(&mut w, ctx)?;
    write_scoring(&mut w, ctx)?;
    Ok(())
}

fn write_inputs(w: &mut Writer<'_>, ctx: &FrameContext<'_>) -> Result<(), SyntheticError> {
    let d = ctx.dynamics;
    let v = ctx.vehicle;
    w.f32(ch::THROTTLE, d.throttle)?;
    w.f32(ch::BRAKE, d.brake)?;
    w.f32(ch::CLUTCH, d.clutch)?;
    w.f32(ch::STEERING, d.steering)?;
    w.f32(ch::THROTTLE_FILTERED, d.throttle_filtered)?;
    w.f32(ch::BRAKE_FILTERED, d.brake_filtered)?;
    w.f32(ch::CLUTCH_FILTERED, d.clutch)?;
    w.f32(ch::STEERING_FILTERED, d.steering)?;
    w.f32(ch::STEERING_TORQUE, d.steering_torque_nm)?;
    w.f32(ch::STEERING_RANGE_PHYSICAL, v.steering_lock_rad)?;
    w.f32(ch::STEERING_RANGE_VISUAL, v.steering_lock_rad)?;
    // Normalizer formula: input × half the lock-to-lock range.
    w.f32(ch::STEERING_ANGLE, d.steering * v.steering_lock_rad / 2.0)?;
    w.boolean(ch::TC_ACTIVE, d.tc_active)?;
    w.gated_setting(ch::TC_LEVEL, v.tc_level, ch::TC_LEVEL_MAX, v.tc_level_max)?;
    w.gated_setting(ch::TC_SLIP, v.tc_level, ch::TC_SLIP_MAX, v.tc_level_max)?;
    w.gated_setting(ch::TC_CUT, v.tc_level, ch::TC_CUT_MAX, v.tc_level_max)?;
    w.gated_setting(
        ch::ABS_LEVEL,
        v.abs_level,
        ch::ABS_LEVEL_MAX,
        v.abs_level_max,
    )?;
    if v.abs_level_max > 0 {
        w.boolean(ch::ABS_ACTIVE, d.abs_active)?;
    } else {
        w.not_supported(ch::ABS_ACTIVE)?;
    }
    w.gated_setting(
        ch::ARB_FRONT,
        ARB_SETTING,
        ch::ARB_FRONT_MAX,
        ARB_SETTING_MAX,
    )?;
    w.gated_setting(ch::ARB_REAR, ARB_SETTING, ch::ARB_REAR_MAX, ARB_SETTING_MAX)?;
    let wx = ctx.weather;
    w.boolean(
        ch::HEADLIGHTS,
        wx.dark_cloud > HEADLIGHT_DARK_CLOUD || wx.rain > HEADLIGHT_RAIN,
    )?;
    let wiper = if wx.rain > WIPER_FAST_RAIN {
        2
    } else if wx.rain > WIPER_SLOW_RAIN {
        1
    } else {
        0
    };
    w.u8(ch::WIPER_STATE, wiper)
}

fn write_vehicle(w: &mut Writer<'_>, ctx: &FrameContext<'_>) -> Result<(), SyntheticError> {
    let v = ctx.vehicle;
    w.i32(ch::VEHICLE_SLOT_ID, ctx.car.slot_id)?;
    w.text(ch::VEHICLE_NAME, &v.name)?;
    w.text(ch::VEHICLE_MODEL, &v.model)?;
    w.u8(ch::VEHICLE_CLASS, v.class.raw())?;
    w.u8(ch::VEHICLE_CHAMPIONSHIP, v.championship.raw())?;
    w.text(ch::VEHICLE_CLASS_NAME, &v.class_name)?;
    w.text(ch::VEHICLE_FILENAME, &v.filename)?;
    for k in 0..DENT_LOCATIONS {
        w.u8(ChannelId(ch::DENT_SEVERITY_0.0 + k), 0)?;
    }
    w.boolean(ch::PARTS_DETACHED, false)?;
    for i in 0..WHEELS {
        w.boolean(wheel(ch::WHEEL_DETACHED_FL, i), false)?;
        w.boolean(wheel(ch::TYRE_FLAT_FL, i), false)?;
    }
    Ok(())
}

fn write_powertrain(w: &mut Writer<'_>, ctx: &FrameContext<'_>) -> Result<(), SyntheticError> {
    let d = ctx.dynamics;
    let v = ctx.vehicle;
    w.i32(ch::GEAR, d.gear)?;
    w.f32(ch::RPM, d.rpm)?;
    w.f32(ch::RPM_MAX, v.max_rpm)?;
    w.opt_u8(ch::GEARS_MAX, u8::try_from(v.gear_count()).ok())?;
    w.f32(ch::CLUTCH_RPM, d.rpm)?;
    let forces_filled = ctx.channel_fill.tyre_forces_filled(v.kind);
    if forces_filled {
        w.f32(ch::ENGINE_TORQUE, d.engine_torque_nm)?;
    } else {
        w.unavailable(ch::ENGINE_TORQUE)?;
    }
    w.f32(ch::WATER_TEMP, d.water_temp_c)?;
    w.f32(ch::OIL_TEMP, d.oil_temp_c)?;
    if v.turbo && !ctx.channel_fill.turbo_filled(v.kind) {
        w.unavailable(ch::TURBO_BOOST)?;
    } else if v.turbo {
        w.f32(ch::TURBO_BOOST, d.turbo_boost_pa)?;
    } else {
        w.not_supported(ch::TURBO_BOOST)?;
    }
    w.boolean(ch::OVERHEATING, false)?;
    w.u8(ch::IGNITION_STARTER, IgnitionStarter::Ignition.raw())?;
    w.boolean(ch::ANTI_STALL, false)?;
    w.boolean(ch::SPEED_LIMITER, d.limiter_on)?;
    w.boolean(ch::SPEED_LIMITER_ACTIVE, d.limiter_active)?;
    w.boolean(ch::SPEED_LIMITER_AVAILABLE, true)?;
    // Normalizer formula: torque × angular speed; withheld with the torque.
    if forces_filled {
        w.f32(ch::ENGINE_POWER, d.engine_torque_nm * rpm_to_radps(d.rpm))?;
    } else {
        w.unavailable(ch::ENGINE_POWER)?;
    }
    w.f32(ch::FUEL, d.fuel_l)?;
    w.f32(ch::FUEL_CAPACITY, v.fuel_capacity_l)?;
    w.f32(ch::FUEL_FRACTION, d.fuel_l / v.fuel_capacity_l)
}

fn write_tyres_and_brakes(
    w: &mut Writer<'_>,
    ctx: &FrameContext<'_>,
) -> Result<(), SyntheticError> {
    let d = ctx.dynamics;
    let v = ctx.vehicle;
    let forces_filled = ctx.channel_fill.tyre_forces_filled(v.kind);
    let compound_name = d.compound.label();
    w.text(ch::TYRE_COMPOUND_FRONT, compound_name)?;
    w.text(ch::TYRE_COMPOUND_REAR, compound_name)?;
    w.u8(ch::TYRE_COMPOUND_INDEX_FRONT, d.compound.raw())?;
    w.u8(ch::TYRE_COMPOUND_INDEX_REAR, d.compound.raw())?;
    w.f32(ch::BRAKE_BIAS_REAR, v.brake_bias_rear)?;
    // Brake migration exists on the hybrid profiles only; a maximum of 0 means "no such system".
    let migration_max = if v.hybrid { BRAKE_MIGRATION_MAX } else { 0 };
    w.gated_setting(
        ch::BRAKE_MIGRATION,
        BRAKE_MIGRATION,
        ch::BRAKE_MIGRATION_MAX,
        migration_max,
    )?;
    for i in 0..WHEELS {
        let [left, middle, right] = d.tyre_surface_c[i];
        let carcass = d.tyre_carcass_c[i];
        w.f32(wheel(ch::TYRE_PRESSURE_FL, i), d.tyre_pressure_pa(i))?;
        w.f32(wheel(ch::TYRE_TEMP_LEFT_FL, i), left)?;
        w.f32(wheel(ch::TYRE_TEMP_MIDDLE_FL, i), middle)?;
        w.f32(wheel(ch::TYRE_TEMP_RIGHT_FL, i), right)?;
        w.f32(wheel(ch::TYRE_LAYER_TEMP_LEFT_FL, i), layer(carcass, left))?;
        w.f32(
            wheel(ch::TYRE_LAYER_TEMP_MIDDLE_FL, i),
            layer(carcass, middle),
        )?;
        w.f32(
            wheel(ch::TYRE_LAYER_TEMP_RIGHT_FL, i),
            layer(carcass, right),
        )?;
        w.f32(wheel(ch::TYRE_CARCASS_TEMP_FL, i), carcass)?;
        w.f32(wheel(ch::TYRE_OPTIMAL_TEMP_FL, i), v.tyre_optimal_temp_c)?;
        w.f32(wheel(ch::TYRE_WEAR_FL, i), d.tyre_wear[i])?;
        let forces = [
            (ch::TYRE_SLIDING_FL, d.tyre_sliding[i]),
            (ch::TYRE_LOAD_FL, d.tyre_load_n[i]),
            (ch::TYRE_LAT_FORCE_FL, d.tyre_lat_force_n[i]),
            (ch::TYRE_LONG_FORCE_FL, d.tyre_long_force_n[i]),
        ];
        for (base, value) in forces {
            if forces_filled {
                w.f32(wheel(base, i), value)?;
            } else {
                w.unavailable(wheel(base, i))?;
            }
        }
        w.f32(wheel(ch::TYRE_RADIUS_FL, i), v.tyre_radius_m)?;
        w.u8(wheel(ch::TYRE_COMPOUND_INDEX_FL, i), d.compound.raw())?;
        w.u8(wheel(ch::TYRE_COMPOUND_TYPE_FL, i), d.compound.raw())?;
        w.u8(wheel(ch::SURFACE_TYPE_FL, i), d.surface[i].raw())?;
        let terrain = if d.surface[i] == telemetry_model::enums::SurfaceType::Grass {
            TERRAIN_GRASS
        } else {
            TERRAIN_ROAD
        };
        w.text(wheel(ch::TERRAIN_NAME_FL, i), terrain)?;
        // Normalizer formulas: inner/outer by wheel side (left wheels: inner = right edge).
        let (inner, outer) = if i % 2 == 0 {
            (right, left)
        } else {
            (left, right)
        };
        w.f32(wheel(ch::TYRE_TEMP_INNER_FL, i), inner)?;
        w.f32(wheel(ch::TYRE_TEMP_OUTER_FL, i), outer)?;
        w.f32(
            wheel(ch::TYRE_TEMP_AVG_FL, i),
            (left + middle + right) / 3.0,
        )?;
        w.f32(wheel(ch::TYRE_TEMP_SPREAD_FL, i), inner - outer)?;
        w.f32(wheel(ch::BRAKE_TEMP_FL, i), d.brake_temp_c[i])?;
        w.f32(wheel(ch::BRAKE_PRESSURE_FL, i), d.brake_pressure[i])?;
        // Suspension and wheels.
        w.f32(wheel(ch::SUSP_POS_FL, i), d.susp_deflection_m[i])?;
        w.f32(wheel(ch::RIDE_HEIGHT_FL, i), d.ride_height_m[i])?;
        w.f32(wheel(ch::CAMBER_FL, i), d.camber_rad[i])?;
        w.f32(wheel(ch::TOE_FL, i), d.toe_rad[i])?;
        w.f32(wheel(ch::WHEEL_ROT_FL, i), d.wheel_rot_radps[i])?;
        // Normalizer formula: |rotation| × static radius.
        w.f32(
            wheel(ch::WHEEL_SPEED_FL, i),
            d.wheel_rot_radps[i].abs() * v.tyre_radius_m,
        )?;
    }
    Ok(())
}

/// Inner-layer temperature between carcass and surface.
fn layer(carcass_c: f64, surface_c: f64) -> f64 {
    carcass_c + 0.5 * (surface_c - carcass_c)
}

fn write_body(w: &mut Writer<'_>, ctx: &FrameContext<'_>) -> Result<(), SyntheticError> {
    let d = ctx.dynamics;
    let [vx, vy, vz] = d.local_vel;
    let [ax, ay, az] = d.local_accel;
    let [rx, ry, rz] = d.local_rot;
    let [rax, ray, raz] = d.local_rot_accel;
    w.f32(ch::LOCAL_VEL_X, vx)?;
    w.f32(ch::LOCAL_VEL_Y, vy)?;
    w.f32(ch::LOCAL_VEL_Z, vz)?;
    w.f32(ch::LOCAL_ACCEL_X, ax)?;
    w.f32(ch::LOCAL_ACCEL_Y, ay)?;
    w.f32(ch::LOCAL_ACCEL_Z, az)?;
    w.f32(ch::LOCAL_ROT_X, rx)?;
    w.f32(ch::LOCAL_ROT_Y, ry)?;
    w.f32(ch::LOCAL_ROT_Z, rz)?;
    w.f32(ch::LOCAL_ROT_ACCEL_X, rax)?;
    w.f32(ch::LOCAL_ROT_ACCEL_Y, ray)?;
    w.f32(ch::LOCAL_ROT_ACCEL_Z, raz)?;
    // Orientation rows map local axes to world (A-LMU-025): local x (left) = (−cos h, 0, sin h),
    // local y = up, local z (rear) = −forward = (−sin h, 0, −cos h); flat car.
    let (sh, chh) = ctx.heading_rad.sin_cos();
    let rows = [[-chh, 0.0, -sh], [0.0, 1.0, 0.0], [sh, 0.0, -chh]];
    let ids = [
        [
            ch::ORIENTATION_R0_X,
            ch::ORIENTATION_R0_Y,
            ch::ORIENTATION_R0_Z,
        ],
        [
            ch::ORIENTATION_R1_X,
            ch::ORIENTATION_R1_Y,
            ch::ORIENTATION_R1_Z,
        ],
        [
            ch::ORIENTATION_R2_X,
            ch::ORIENTATION_R2_Y,
            ch::ORIENTATION_R2_Z,
        ],
    ];
    for (row_ids, row) in ids.iter().zip(rows) {
        for (id, value) in row_ids.iter().zip(row) {
            w.f32(*id, value)?;
        }
    }
    // Normalizer formulas (convention.rs): ISO X = −z, Y = +x, Z = +y; yaw = −rot.y, pitch = −rot.x.
    let speed = convert::magnitude(vx, vy, vz);
    let g_lat = convert::accel_to_g(ax);
    let g_long = convert::accel_to_g(-az);
    w.f32(ch::SPEED, speed)?;
    w.f32(ch::SPEED_FORWARD, -vz)?;
    w.f32(ch::G_LAT, g_lat)?;
    w.f32(ch::G_LONG, g_long)?;
    w.f32(ch::G_VERT, convert::accel_to_g(ay))?;
    w.f32(ch::G_COMBINED, g_lat.hypot(g_long))?;
    w.f32(ch::YAW_RATE, -ry)?;
    w.f32(ch::PITCH_RATE, -rx)?;
    w.f32(ch::ROLL_RATE, rz)?;
    let aero = [
        (ch::RIDE_HEIGHT_FRONT, d.ride_front_m),
        (ch::RIDE_HEIGHT_REAR, d.ride_rear_m),
        (ch::DRAG, d.drag_n),
        (ch::DOWNFORCE_FRONT, d.downforce_front_n),
        (ch::DOWNFORCE_REAR, d.downforce_rear_n),
    ];
    for (id, value) in aero {
        if ctx.channel_fill.aero_filled() {
            w.f32(id, value)?;
        } else {
            w.unavailable(id)?;
        }
    }
    let (x, y, z) = ctx.position;
    w.f64(ch::POS_X, x)?;
    w.f64(ch::POS_Y, y)?;
    w.f64(ch::POS_Z, z)?;
    let limit = PI - HEADING_F32_MARGIN_RAD;
    w.f32(ch::HEADING, wrap_pi(ctx.heading_rad).clamp(-limit, limit))?;
    w.f64(ch::MAP_X, x)?;
    w.f64(ch::MAP_Y, z)
}

fn write_energy(w: &mut Writer<'_>, ctx: &FrameContext<'_>) -> Result<(), SyntheticError> {
    let d = ctx.dynamics;
    let v = ctx.vehicle;
    if v.virtual_energy {
        w.f32(ch::VIRTUAL_ENERGY, d.virtual_energy)?;
    } else {
        w.not_supported(ch::VIRTUAL_ENERGY)?;
    }
    if v.hybrid {
        w.f32(ch::BATTERY_SOC, d.battery_soc)?;
        w.f32(ch::BATTERY_CHARGE, d.battery_soc)?;
        w.f32(ch::REGEN_POWER, d.regen_power_w)?;
        w.f32(ch::HYBRID_MOTOR_TORQUE, d.motor_torque_nm)?;
        w.f32(ch::HYBRID_MOTOR_RPM, d.motor_rpm)?;
        w.f32(ch::HYBRID_MOTOR_TEMP, d.motor_temp_c)?;
        w.f32(ch::HYBRID_WATER_TEMP, d.motor_water_temp_c)?;
        w.u8(ch::HYBRID_MOTOR_STATE, d.motor_state.raw())?;
        w.gated_setting(ch::MOTOR_MAP, MOTOR_MAP, ch::MOTOR_MAP_MAX, MOTOR_MAP_MAX)?;
    } else {
        // No hybrid system: the state reads "Unavailable" and every other hybrid channel NOT_SUPPORTED.
        w.u8(ch::HYBRID_MOTOR_STATE, HybridMotorState::Unavailable.raw())?;
        for id in [
            ch::BATTERY_SOC,
            ch::BATTERY_CHARGE,
            ch::REGEN_POWER,
            ch::HYBRID_MOTOR_TORQUE,
            ch::HYBRID_MOTOR_RPM,
            ch::HYBRID_MOTOR_TEMP,
            ch::HYBRID_WATER_TEMP,
        ] {
            w.not_supported(id)?;
        }
        w.gated_setting(ch::MOTOR_MAP, MOTOR_MAP, ch::MOTOR_MAP_MAX, 0)?;
    }
    Ok(())
}

fn write_timing(w: &mut Writer<'_>, ctx: &FrameContext<'_>) -> Result<(), SyntheticError> {
    let car = ctx.car;
    let track = ctx.track;
    w.f64(ch::ELAPSED_TIME, ctx.et_s)?;
    w.f32(ch::UPDATE_DELTA_TIME, ctx.dt_s)?;
    w.opt_i32(ch::LAP_NUMBER, i32::try_from(car.current_lap()).ok())?;
    w.opt_f64(ch::LAP_START_TIME, car.timing.lap_start_et_s)?;
    w.boolean(ch::LAP_INVALID, !car.timing.lap_valid)?;
    w.opt_f64(ch::DELTA_BEST, ctx.delta_best_s)?;
    w.opt_f64(
        ch::LAP_TIME,
        car.timing.lap_start_et_s.map(|s| (ctx.et_s - s).max(0.0)),
    )?;
    let gaps = &ctx.scoring.gaps;
    w.opt_f32(ch::GAP_CAR_AHEAD, gaps.car_ahead_s)?;
    w.opt_f32(ch::GAP_CAR_BEHIND, gaps.car_behind_s)?;
    w.opt_f32(ch::GAP_PLACE_AHEAD, gaps.place_ahead_s)?;
    w.opt_f32(ch::GAP_PLACE_BEHIND, gaps.place_behind_s)?;
    let length = track.length_m();
    w.f64(ch::TRACK_LENGTH, length)?;
    w.f32(
        ch::LAP_PROGRESS,
        (car.s_m / length).clamp(0.0, 1.0 - f64::EPSILON),
    )?;
    let sector = track.sector_index(car.s_m);
    let pit_bit = if car.pit.in_pit_lane() {
        CURRENT_SECTOR_PIT_BIT
    } else {
        0
    };
    let raw_sector = i32::from_le_bytes((u32::from(sector) | pit_bit).to_le_bytes());
    w.i32(ch::SECTOR_RAW, raw_sector)?;
    w.u8(ch::SECTOR, sector)?;
    w.boolean(ch::IN_PIT_LANE, car.pit.in_pit_lane())?;
    w.u8(ch::TRACK_LIMITS_STEPS, ctx.dynamics.track_limits_steps)?;
    w.boolean(ch::PLAYER_HAS_VEHICLE, true)?;
    w.opt_u8(
        ch::PLAYER_SLOT_INDEX,
        u8::try_from(ctx.scoring.player_index).ok(),
    )?;
    w.opt_u8(
        ch::NUM_VEHICLES_TELEMETRY,
        u8::try_from(ctx.vehicle_count).ok(),
    )
}

/// Scoring-sourced channels: from the latest scoring snapshot (refreshed at the scoring rate).
fn write_scoring(w: &mut Writer<'_>, ctx: &FrameContext<'_>) -> Result<(), SyntheticError> {
    let snap = ctx.scoring;
    let s = &snap.state.session;
    let Some(row) = snap.player() else {
        return Ok(());
    };
    w.opt_f64(ch::BEST_LAP_TIME, row.best_lap_s)?;
    w.opt_f64(ch::LAST_LAP_TIME, row.last_lap_s)?;
    w.opt_f64(ch::BEST_SECTOR1, row.best_sector1_s)?;
    w.opt_f64(ch::BEST_SECTOR12, row.best_sector12_s)?;
    w.opt_f64(ch::LAST_SECTOR1, row.last_sector1_s)?;
    w.opt_f64(ch::LAST_SECTOR12, row.last_sector12_s)?;
    w.opt_f64(ch::CUR_SECTOR1, row.cur_sector1_s)?;
    w.opt_f64(ch::CUR_SECTOR12, row.cur_sector12_s)?;
    let timing = &ctx.car.timing;
    w.opt_f32(ch::BEST_LAP_SECTOR1, timing.best_lap_sector1_s)?;
    w.opt_f32(ch::BEST_LAP_SECTOR12, timing.best_lap_sector12_s)?;
    w.opt_f64(ch::TIME_INTO_LAP, row.time_into_lap_s)?;
    w.opt_f64(ch::ESTIMATED_LAP_TIME, row.estimated_lap_s)?;
    w.opt_f64(ch::LAP_START_TIME_SCORING, row.lap_start_et_s)?;
    // Normalizer-derived sector splits.
    w.opt_f64(ch::LAST_SECTOR2, row.last_sector2_s())?;
    w.opt_f64(ch::LAST_SECTOR3, row.last_sector3_s())?;
    w.opt_f64(ch::BEST_SECTOR2, row.best_sector2_s())?;

    if let Some(t) = s.session_type {
        w.i32(ch::SESSION_TYPE, t.raw())?;
    }
    w.opt_f64(ch::SESSION_TIME, s.current_et_s)?;
    w.opt_f64(ch::SESSION_END_TIME, s.end_et_s)?;
    w.opt_f32(ch::SESSION_TIME_REMAINING, s.time_remaining_s)?;
    w.opt_f64(ch::SESSION_TIME_REMAINING_CALC, s.time_remaining_calc_s())?;
    w.opt_i32(
        ch::NUM_VEHICLES,
        i32::try_from(snap.state.vehicles.len()).ok(),
    )?;
    if let Some(p) = s.game_phase {
        w.u8(ch::GAME_PHASE, p.raw())?;
    }
    w.opt_bool(ch::IN_REALTIME, s.in_realtime)?;
    w.opt_u8(ch::START_LIGHT, s.start_light)?;
    if let Some(n) = s.num_red_lights {
        w.u8(ch::NUM_RED_LIGHTS, n)?;
    }
    if let Some(tod) = s.time_of_day_s {
        let start = (tod - snap.et_s).rem_euclid(SECONDS_PER_DAY);
        w.f32(ch::START_TIME_OF_DAY, start)?;
        w.f32(ch::TIME_OF_DAY, tod)?;
    }
    w.opt_bool(ch::FIXED_SETUP, s.fixed_setup)?;
    if let Some(m) = s.game_mode {
        w.u8(ch::GAME_MODE, m.raw())?;
    }
    if let Some(m) = s.max_players {
        w.i32(ch::MAX_PLAYERS, m)?;
    }
    w.u8(
        ch::TRACK_LIMITS_STEPS_PER_PENALTY,
        TRACK_LIMITS_STEPS_PER_PENALTY,
    )?;
    w.u8(
        ch::TRACK_LIMITS_STEPS_PER_POINT,
        TRACK_LIMITS_STEPS_PER_POINT,
    )?;
    if let Some(name) = &s.track_name {
        w.text(ch::TRACK_NAME, name)?;
    }

    if let Some(p) = row.place {
        w.u8(ch::POSITION, p)?;
    }
    if let Some(p) = row.class_place {
        w.u8(ch::CLASS_POSITION, p)?;
    }
    if let Some(l) = row.laps {
        w.i32(ch::LAPS_COMPLETED, l)?;
    }
    if let Some(f) = row.finish_status {
        w.i8(ch::FINISH_STATUS, f.raw())?;
    }
    w.opt_f64(ch::TIME_BEHIND_NEXT, row.time_behind_next_s)?;
    if let Some(l) = row.laps_behind_next {
        w.i32(ch::LAPS_BEHIND_NEXT, l)?;
    }
    w.opt_f64(ch::TIME_BEHIND_LEADER, row.time_behind_leader_s)?;
    if let Some(l) = row.laps_behind_leader {
        w.i32(ch::LAPS_BEHIND_LEADER, l)?;
    }
    if let Some(q) = row.qualification {
        w.i32(ch::QUALIFYING_POSITION, q)?;
    }
    if let Some(c) = row.control {
        w.i8(ch::CONTROL, c.raw())?;
    }
    w.boolean(ch::SERVER_SCORED, false)?;
    w.opt_u8(
        ch::COUNT_LAP_FLAG,
        row.count_lap_flag.map(CountLapFlag::raw),
    )?;
    if let Some(p) = row.num_penalties {
        w.i32(ch::PENALTIES, p)?;
    }

    let wx = &s.weather;
    w.opt_f32(ch::AIR_TEMP, wx.ambient_temp_c)?;
    w.opt_f32(ch::TRACK_TEMP, wx.track_temp_c)?;
    w.opt_f32(ch::RAIN, wx.raining_frac)?;
    w.opt_f32(ch::DARK_CLOUD, wx.dark_cloud_frac)?;
    if let Some(c) = wx.cloud_coverage_raw {
        w.u8(ch::CLOUD_COVERAGE, c)?;
    }
    w.opt_f32(ch::TRACK_WETNESS_MIN, wx.path_wetness_min_frac)?;
    w.opt_f32(ch::TRACK_WETNESS_MAX, wx.path_wetness_max_frac)?;
    w.opt_f32(ch::TRACK_WETNESS, wx.path_wetness_avg_frac)?;
    if let Some([x, y, z]) = wx.wind_mps {
        w.f32(ch::WIND_X, x)?;
        w.f32(ch::WIND_Y, y)?;
        w.f32(ch::WIND_Z, z)?;
    }

    w.opt_f64(ch::LAP_DISTANCE, row.lap_dist_m)?;
    // Like production (scoring row, A-LMU-070): laps × length + lap distance, negative before the first
    // crossing and continuous across it.
    let distance = row
        .laps
        .zip(row.lap_dist_m)
        .map(|(laps, lap_dist)| f64::from(laps) * ctx.track.length_m() + lap_dist);
    w.opt_f64(ch::DISTANCE, distance)?;
    if let Some(sector) = row.sector {
        // Scoring sector is 1-based here; raw mSector is 0 = S3, 1 = S1, 2 = S2.
        let raw = match sector {
            1 => Some(1),
            2 => Some(2),
            3 => Some(SCORING_SECTOR_RAW_S3),
            _ => None,
        };
        match raw {
            Some(raw) => {
                w.i8(ch::SECTOR_SCORING_RAW, raw)?;
                w.u8(ch::SECTOR_SCORING, sector - 1)?;
            }
            None => {
                w.unavailable(ch::SECTOR_SCORING_RAW)?;
                w.unavailable(ch::SECTOR_SCORING)?;
            }
        }
    }
    w.f32(ch::PATH_LATERAL, ctx.lateral_m)?;
    w.f32(ch::TRACK_EDGE, ctx.track.half_width_m())?;

    if let Some(y) = s.yellow_flag_state {
        w.i8(ch::YELLOW_FLAG_STATE, y.raw())?;
        // Normalizer rule: a full-course caution is pending … resume (raw 1..=6).
        let fcy = matches!(
            y,
            YellowFlagState::Pending
                | YellowFlagState::PitsClosed
                | YellowFlagState::PitLeadLap
                | YellowFlagState::PitsOpen
                | YellowFlagState::LastLap
                | YellowFlagState::Resume
        );
        w.boolean(ch::FULL_COURSE_YELLOW, fcy)?;
    }
    if let Some(flags) = s.sector_flags_raw {
        w.i8(ch::SECTOR_FLAG_0, flags[0])?;
        w.i8(ch::SECTOR_FLAG_1, flags[1])?;
        w.i8(ch::SECTOR_FLAG_2, flags[2])?;
    }
    if let Some(f) = row.flag {
        w.u8(ch::CAR_FLAG, f.raw())?;
        w.boolean(ch::BLUE_FLAG, f == telemetry_model::enums::CarFlag::Blue)?;
    }
    if let Some(u) = row.under_yellow {
        w.boolean(ch::UNDER_YELLOW, u)?;
    }
    if let Some(p) = row.individual_phase {
        w.u8(ch::INDIVIDUAL_PHASE, p.raw())?;
    }
    w.opt_bool(ch::IN_PITS, row.in_pits)?;
    if let Some(p) = row.pit_state {
        w.u8(ch::PIT_STATE, p.raw())?;
    }
    w.opt_bool(ch::IN_GARAGE, row.in_garage_stall)?;
    w.f32(ch::PIT_LAP_DISTANCE, ctx.track.pit().box_s_m)?;
    if let Some(n) = row.num_pitstops {
        w.i32(ch::PIT_STOPS, n)?;
    }
    Ok(())
}

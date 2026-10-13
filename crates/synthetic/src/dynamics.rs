//! Detailed player-car state derived from the kinematics each tick: pedals, gears and engine, hybrid
//! system and energy, fuel, tyres (loads, forces, temperatures, pressures, wear), brakes, fluids, body
//! motion in LMU's local frame and the pit-stop service (refuel and tyre change, ramped so the registry's
//! rate limits hold).
//!
//! Body-frame conventions are those of lmu-normalizer `convention.rs` (A-LMU-024): LMU local +x points out
//! of the car's left side, +y up, +z to the rear; +rot.y yaws right. Published G channels: G_LAT + = left,
//! G_LONG + = accelerating.

use rand::Rng;
use rand_chacha::ChaCha8Rng;
use telemetry_model::enums::{CompoundType, HybridMotorState, SurfaceType};
use telemetry_model::units::{KELVIN_OFFSET, STANDARD_GRAVITY};

use crate::profile::SpeedProfile;
use crate::track::{Track, TrackPoint};
use crate::vehicle::{
    ENGINE_BRAKE_DECEL_MPS2, ROLLING_DECEL_MPS2, VehicleProfile, radps_to_rpm, rpm_to_radps,
};

/// Wheel order used everywhere: FL, FR, RL, RR (registry stride order).
pub const WHEELS: usize = 4;
/// Tyre surface zones: left, middle, right.
pub const ZONES: usize = 3;

/// Pedal response time constant (s).
const PEDAL_TAU_S: f64 = 0.04;
/// Pedal noise amplitude while partially pressed.
const PEDAL_NOISE: f64 = 0.01;
/// Steering random-walk step and bound (fraction of full lock).
const STEERING_NOISE_STEP: f64 = 0.0008;
const STEERING_NOISE_BOUND: f64 = 0.01;
/// Understeer added to the road-wheel angle per g of lateral acceleration (rad/g).
const UNDERSTEER_RAD_PER_G: f64 = 0.004;
/// Steering shaft torque per m/s² of lateral acceleration (N·m).
const STEERING_TORQUE_PER_MPS2: f64 = 1.2;
/// Sensor noise of the accelerations (m/s², 1 σ).
const ACCEL_NOISE_LAT: f64 = 0.25;
const ACCEL_NOISE_LONG: f64 = 0.2;
const ACCEL_NOISE_VERT: f64 = 0.5;
/// Angular-rate noise (rad/s, 1 σ).
const ROT_NOISE: f64 = 0.004;
/// Body slip velocity per g of lateral acceleration (fraction of speed).
const BODY_SLIP_PER_G: f64 = 0.004;
/// Launch: clutch fully released above this speed (m/s); engine speed held while slipping (rpm share).
const LAUNCH_SPEED_MPS: f64 = 8.0;
const LAUNCH_RPM_SHARE: f64 = 0.45;
/// Engine drag torque with the throttle closed (N·m).
const ENGINE_DRAG_TORQUE_NM: f64 = 60.0;
/// Idle fuel share of the consumption weight.
const IDLE_FUEL_SHARE: f64 = 0.03;
/// Lap-to-lap fuel factor clamp.
const FUEL_FACTOR_RANGE: (f64, f64) = (0.9, 1.1);
/// Battery state-of-charge band (fraction).
const SOC_MIN: f64 = 0.2;
const SOC_MAX: f64 = 0.95;
/// Hybrid regeneration power at full brake (W) and efficiency.
const REGEN_MAX_W: f64 = 200_000.0;
const REGEN_EFFICIENCY: f64 = 0.85;
/// Hybrid deployment needs at least this throttle.
const DEPLOY_MIN_THROTTLE: f64 = 0.8;
/// Hybrid motor/water temperature model: base, rise at full power, time constants.
const MOTOR_TEMP_BASE_C: f64 = 55.0;
const MOTOR_TEMP_RISE_C: f64 = 35.0;
const MOTOR_TEMP_TAU_S: f64 = 40.0;
const MOTOR_WATER_BASE_C: f64 = 45.0;
const MOTOR_WATER_RISE_C: f64 = 15.0;
const MOTOR_WATER_TAU_S: f64 = 90.0;
/// TC / ABS intervention probability per tick when triggered, and hold time (s).
const AID_TRIGGER_PROBABILITY: f64 = 0.03;
const AID_HOLD_S: f64 = 0.1;
/// Turbo boost (absolute, Pa): atmospheric plus boost at full throttle.
const TURBO_BASE_PA: f64 = 101_325.0;
const TURBO_BOOST_PA: f64 = 140_000.0;
/// Fluid temperatures: load rise, wet cooling, ambient sensitivity, time constant, start offsets.
const WATER_LOAD_RISE_C: f64 = 8.0;
const OIL_LOAD_RISE_C: f64 = 10.0;
const FLUID_WET_COOLING_C: f64 = 3.0;
const FLUID_AMBIENT_GAIN: f64 = 0.3;
const FLUID_AMBIENT_REF_C: f64 = 22.0;
const FLUID_TAU_S: f64 = 40.0;
const FLUID_STATIONARY_RISE_C: f64 = 6.0;
const WATER_START_OFFSET_C: f64 = -8.0;
const OIL_START_OFFSET_C: f64 = -10.0;
const THROTTLE_AVG_TAU_S: f64 = 10.0;
/// Tyre thermal model.
const CARCASS_TAU_S: f64 = 45.0;
const SURFACE_TAU_S: f64 = 1.0;
const CARCASS_SPEED_RISE_C: f64 = 15.0;
const CARCASS_SPEED_REF_MPS: f64 = 60.0;
const CARCASS_WORK_RISE_C: f64 = 55.0;
const CARCASS_WET_COOLING_C: f64 = 25.0;
const SURFACE_WORK_RISE_C: f64 = 25.0;
const SURFACE_CORNER_EDGE_C: f64 = 6.0;
const CAMBER_INNER_C: f64 = 4.0;
const CAMBER_MIDDLE_C: f64 = 1.0;
const CAMBER_OUTER_C: f64 = -2.0;
const UTIL_AVG_TAU_S: f64 = 20.0;
/// Tyre utilisation clamp and the sliding onset.
const UTIL_MAX: f64 = 1.5;
const SLIDING_ONSET: f64 = 0.9;
const SLIDING_SPAN: f64 = 0.3;
/// Wear per metre at utilisation² + base (fraction / m).
const WEAR_PER_M: f64 = 4.6e-6;
const WEAR_BASE: f64 = 0.2;
/// Reference temperature of the cold pressure (°C).
const COLD_PRESSURE_REF_C: f64 = 20.0;
/// Fresh tyres from the garage are this much warmer than the air (°C).
const FRESH_TYRE_ABOVE_AIR_C: f64 = 8.0;
/// Pit service: start of the tyre swap after stopping, its ramp, fixed overhead, wheel change time,
/// refuel rate (s, s, s, s, L/s).
const TYRE_SWAP_START_S: f64 = 1.0;
const TYRE_SWAP_RAMP_S: f64 = 1.0;
const SERVICE_OVERHEAD_S: f64 = 2.0;
const TYRE_CHANGE_S: f64 = 10.0;
const REFUEL_RATE_LPS: f64 = 2.5;
/// Brake thermal model: heating per (pressure × m/s), cooling base and speed gain, start temperature.
const BRAKE_HEAT_PER_MPS: f64 = 2.0;
const BRAKE_COOL_BASE: f64 = 0.01;
const BRAKE_COOL_PER_MPS: f64 = 0.0008;
const BRAKE_START_C: f64 = 250.0;
/// Rear brake heating relative to the front at equal pressure.
const REAR_BRAKE_HEAT_SHARE: f64 = 0.8;
/// Suspension: static ride heights (m), aero compression (m/N), pitch (m per m/s²), roll (m per m/s²),
/// wheel rate (N/m), ride-height clamp (m).
const RIDE_FRONT_STATIC_M: f64 = 0.050;
const RIDE_REAR_STATIC_M: f64 = 0.075;
const RIDE_AERO_M_PER_N: f64 = 2.5e-6;
const RIDE_PITCH_M_PER_MPS2: f64 = 0.0006;
const RIDE_ROLL_M_PER_MPS2: f64 = 0.0004;
const WHEEL_RATE_N_PER_M: f64 = 250_000.0;
const RIDE_MIN_M: f64 = 0.005;
const RIDE_MAX_M: f64 = 0.25;
/// Static alignment (rad).
const CAMBER_FRONT_RAD: f64 = -0.052;
const CAMBER_REAR_RAD: f64 = -0.035;
const TOE_FRONT_RAD: f64 = -0.002;
const TOE_REAR_RAD: f64 = 0.003;
/// Driven-wheel slip under traction in low gears and front lock under ABS (fractions).
const TRACTION_SLIP: f64 = 0.03;
const ABS_LOCK_SLIP: f64 = 0.05;
/// Highest gear in which traction slip is modelled.
const TRACTION_SLIP_MAX_GEAR: i32 = 3;
/// Track wetness above which the surface reads as wet.
const WET_SURFACE_WETNESS: f64 = 0.3;
/// Track-limits excursion length (s) and how far beyond the edge the car runs (m).
const EXCURSION_S: f64 = 2.0;
const EXCURSION_BEYOND_EDGE_M: f64 = 1.0;

/// Compound wear multiplier.
fn compound_wear_factor(compound: CompoundType) -> f64 {
    match compound {
        CompoundType::Soft => 1.4,
        CompoundType::Medium | CompoundType::Unknown(_) => 1.0,
        CompoundType::Hard => 0.7,
        CompoundType::Wet => 1.2,
    }
}

/// Standard normal sample (Box–Muller) from the deterministic generator.
pub fn gaussian(rng: &mut ChaCha8Rng) -> f64 {
    let u1: f64 = 1.0 - rng.random::<f64>();
    let u2: f64 = rng.random::<f64>();
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}

/// First-order low-pass step towards `target`.
fn approach(value: f64, target: f64, dt_s: f64, tau_s: f64) -> f64 {
    value + (target - value) * (dt_s / tau_s).min(1.0)
}

/// Pedal positions that produce longitudinal acceleration `a_long` at speed `v` (no filtering).
pub fn pedals_for(vehicle: &VehicleProfile, v_mps: f64, a_long: f64, grip: f64) -> (f64, f64) {
    let drive = a_long + vehicle.drag_decel(v_mps) + ROLLING_DECEL_MPS2;
    if drive >= 0.0 {
        let throttle = drive / vehicle.max_drive_accel(v_mps, grip).max(f64::EPSILON);
        (throttle.clamp(0.0, 1.0), 0.0)
    } else {
        let decel = -drive - ENGINE_BRAKE_DECEL_MPS2;
        if decel <= 0.0 {
            (0.0, 0.0)
        } else {
            let brake = decel / vehicle.max_brake_decel(v_mps, grip).max(f64::EPSILON);
            (0.0, brake.clamp(0.0, 1.0))
        }
    }
}

/// Fuel/energy consumption weight of one tick (throttle × engine-speed share + idle).
fn consumption_weight(throttle: f64, rpm: f64, max_rpm: f64) -> f64 {
    throttle * (rpm / max_rpm) + IDLE_FUEL_SHARE
}

/// Consumption weight of one ideal lap of `profile` (used to calibrate fuel and virtual energy use so a
/// nominal lap consumes the vehicle's nominal amounts).
pub fn lap_consumption_weight(
    track: &Track,
    vehicle: &VehicleProfile,
    profile: &SpeedProfile,
) -> f64 {
    let n = track.station_count();
    let ds = track.spacing_m();
    let mut gear = 1;
    let mut total = 0.0;
    for i in 0..n {
        let s = i as f64 * ds;
        let v0 = profile.speed_at(s);
        let v1 = profile.speed_at(s + ds);
        let v = 0.5 * (v0 + v1);
        let dt = ds / v.max(f64::EPSILON);
        let a = (v1 * v1 - v0 * v0) / (2.0 * ds);
        let (throttle, _) = pedals_for(vehicle, v, a, 1.0);
        gear = vehicle.select_gear(v, gear);
        total += consumption_weight(throttle, vehicle.engine_rpm(v, gear), vehicle.max_rpm) * dt;
    }
    total
}

/// Pit service requested for the current stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ServiceRequest {
    pub refuel_l: Option<f64>,
    pub change_tyres: bool,
    pub compound: Option<CompoundType>,
}

/// Service in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Service {
    elapsed_s: f64,
    fuel_target_l: f64,
    ve_target: f64,
    change_tyres: bool,
    compound: Option<CompoundType>,
    from: Option<TyreSnapshot>,
}

/// Tyre state captured when a swap starts.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TyreSnapshot {
    carcass_c: [f64; WHEELS],
    surface_c: [[f64; ZONES]; WHEELS],
    wear: [f64; WHEELS],
}

/// Environment of one tick.
#[derive(Debug, Clone, Copy)]
pub struct TickInput {
    pub dt_s: f64,
    pub v_mps: f64,
    pub a_long_mps2: f64,
    /// Distance driven this tick (m).
    pub distance_m: f64,
    pub point: TrackPoint,
    pub grip: f64,
    pub wetness: f64,
    pub air_temp_c: f64,
    pub track_temp_c: f64,
    /// Stationary in the pit box.
    pub stopped: bool,
    pub in_pit_lane: bool,
    pub pit_speed_limit_mps: f64,
    /// Half the track width (m).
    pub half_width_m: f64,
}

/// Everything the frame writer needs about the player car.
#[derive(Debug, Clone, PartialEq)]
pub struct Dynamics {
    pub throttle: f64,
    pub brake: f64,
    pub throttle_filtered: f64,
    pub brake_filtered: f64,
    pub clutch: f64,
    pub steering: f64,
    pub steering_torque_nm: f64,
    pub gear: i32,
    pub rpm: f64,
    pub engine_torque_nm: f64,
    pub tc_active: bool,
    pub abs_active: bool,
    pub limiter_on: bool,
    pub limiter_active: bool,
    pub turbo_boost_pa: f64,
    pub water_temp_c: f64,
    pub oil_temp_c: f64,
    pub fuel_l: f64,
    pub virtual_energy: f64,
    pub battery_soc: f64,
    pub regen_power_w: f64,
    pub motor_torque_nm: f64,
    pub motor_rpm: f64,
    pub motor_temp_c: f64,
    pub motor_water_temp_c: f64,
    pub motor_state: HybridMotorState,
    pub compound: CompoundType,
    pub tyre_carcass_c: [f64; WHEELS],
    pub tyre_surface_c: [[f64; ZONES]; WHEELS],
    pub tyre_wear: [f64; WHEELS],
    pub tyre_cold_pressure_pa: f64,
    pub tyre_load_n: [f64; WHEELS],
    pub tyre_lat_force_n: [f64; WHEELS],
    pub tyre_long_force_n: [f64; WHEELS],
    pub tyre_sliding: [f64; WHEELS],
    pub brake_temp_c: [f64; WHEELS],
    pub brake_pressure: [f64; WHEELS],
    pub wheel_rot_radps: [f64; WHEELS],
    pub ride_height_m: [f64; WHEELS],
    pub ride_front_m: f64,
    pub ride_rear_m: f64,
    pub susp_deflection_m: [f64; WHEELS],
    pub camber_rad: [f64; WHEELS],
    pub toe_rad: [f64; WHEELS],
    pub surface: [SurfaceType; WHEELS],
    pub drag_n: f64,
    pub downforce_front_n: f64,
    pub downforce_rear_n: f64,
    /// LMU local frame vectors.
    pub local_vel: [f64; 3],
    pub local_accel: [f64; 3],
    pub local_rot: [f64; 3],
    pub local_rot_accel: [f64; 3],
    /// Lateral offset from the racing line, + = right (m).
    pub lateral_m: f64,
    /// Track-limits steps so far.
    pub track_limits_steps: u8,
    // --- internal state ---
    util_avg: [f64; WHEELS],
    throttle_avg: f64,
    fuel_k_per_weight: f64,
    ve_k_per_weight: f64,
    fuel_factor: f64,
    steering_noise: f64,
    tc_hold_s: f64,
    abs_hold_s: f64,
    excursion_left_s: f64,
    service: Option<Service>,
}

impl Dynamics {
    /// Initial state.
    pub fn new(
        vehicle: &VehicleProfile,
        start: &crate::scenario::StartSpec,
        lap_weight: f64,
        air_temp_c: f64,
    ) -> Self {
        let weight = lap_weight.max(f64::EPSILON);
        let tyre = start.tyre_temp_c;
        Self {
            throttle: 0.0,
            brake: 0.0,
            throttle_filtered: 0.0,
            brake_filtered: 0.0,
            clutch: 0.0,
            steering: 0.0,
            steering_torque_nm: 0.0,
            gear: 1,
            rpm: vehicle.idle_rpm,
            engine_torque_nm: 0.0,
            tc_active: false,
            abs_active: false,
            limiter_on: false,
            limiter_active: false,
            turbo_boost_pa: TURBO_BASE_PA,
            water_temp_c: vehicle.water_temp_nominal_c + WATER_START_OFFSET_C,
            oil_temp_c: vehicle.oil_temp_nominal_c + OIL_START_OFFSET_C,
            fuel_l: start
                .fuel_l
                .unwrap_or(vehicle.fuel_capacity_l)
                .min(vehicle.fuel_capacity_l),
            virtual_energy: start.virtual_energy_frac,
            battery_soc: start.battery_soc_frac,
            regen_power_w: 0.0,
            motor_torque_nm: 0.0,
            motor_rpm: 0.0,
            motor_temp_c: MOTOR_TEMP_BASE_C,
            motor_water_temp_c: MOTOR_WATER_BASE_C,
            motor_state: HybridMotorState::Inactive,
            compound: vehicle.default_compound,
            tyre_carcass_c: [tyre; WHEELS],
            tyre_surface_c: [[tyre; ZONES]; WHEELS],
            tyre_wear: [1.0; WHEELS],
            tyre_cold_pressure_pa: vehicle.tyre_cold_pressure_pa,
            tyre_load_n: [0.0; WHEELS],
            tyre_lat_force_n: [0.0; WHEELS],
            tyre_long_force_n: [0.0; WHEELS],
            tyre_sliding: [0.0; WHEELS],
            brake_temp_c: [BRAKE_START_C.max(air_temp_c); WHEELS],
            brake_pressure: [0.0; WHEELS],
            wheel_rot_radps: [0.0; WHEELS],
            ride_height_m: [
                RIDE_FRONT_STATIC_M,
                RIDE_FRONT_STATIC_M,
                RIDE_REAR_STATIC_M,
                RIDE_REAR_STATIC_M,
            ],
            ride_front_m: RIDE_FRONT_STATIC_M,
            ride_rear_m: RIDE_REAR_STATIC_M,
            susp_deflection_m: [0.0; WHEELS],
            camber_rad: [
                CAMBER_FRONT_RAD,
                CAMBER_FRONT_RAD,
                CAMBER_REAR_RAD,
                CAMBER_REAR_RAD,
            ],
            toe_rad: [TOE_FRONT_RAD, TOE_FRONT_RAD, TOE_REAR_RAD, TOE_REAR_RAD],
            surface: [SurfaceType::Dry; WHEELS],
            drag_n: 0.0,
            downforce_front_n: 0.0,
            downforce_rear_n: 0.0,
            local_vel: [0.0; 3],
            local_accel: [0.0; 3],
            local_rot: [0.0; 3],
            local_rot_accel: [0.0; 3],
            lateral_m: 0.0,
            track_limits_steps: 0,
            util_avg: [0.3; WHEELS],
            throttle_avg: 0.5,
            fuel_k_per_weight: vehicle.fuel_per_lap_l / weight,
            ve_k_per_weight: vehicle.ve_per_lap_frac / weight,
            fuel_factor: 1.0,
            steering_noise: 0.0,
            tc_hold_s: 0.0,
            abs_hold_s: 0.0,
            excursion_left_s: 0.0,
            service: None,
        }
    }

    /// Tyre pressure of `wheel` (Pa): constant-volume ideal gas from the cold setting.
    pub fn tyre_pressure_pa(&self, wheel: usize) -> f64 {
        let t = self
            .tyre_carcass_c
            .get(wheel)
            .copied()
            .unwrap_or(COLD_PRESSURE_REF_C);
        self.tyre_cold_pressure_pa * (t + KELVIN_OFFSET) / (COLD_PRESSURE_REF_C + KELVIN_OFFSET)
    }

    /// Draws the fuel factor of the next lap.
    pub fn new_lap(&mut self, fuel_sigma: f64, rng: &mut ChaCha8Rng) {
        let (lo, hi) = FUEL_FACTOR_RANGE;
        self.fuel_factor = (1.0 + fuel_sigma * gaussian(rng)).clamp(lo, hi);
    }

    /// Starts a track-limits excursion (the caller invalidates the lap).
    pub fn start_excursion(&mut self) {
        self.excursion_left_s = EXCURSION_S;
        self.track_limits_steps = self.track_limits_steps.saturating_add(1);
    }

    /// Starts the pit service and returns its duration (s).
    pub fn begin_service(&mut self, vehicle: &VehicleProfile, request: ServiceRequest) -> f64 {
        let fuel_target = match request.refuel_l {
            None => vehicle.fuel_capacity_l,
            Some(add) => (self.fuel_l + add).min(vehicle.fuel_capacity_l),
        };
        let added = (fuel_target - self.fuel_l).max(0.0);
        let ve_target = if request.refuel_l.is_none() {
            1.0
        } else {
            (self.virtual_energy + added * vehicle.ve_per_lap_frac / vehicle.fuel_per_lap_l)
                .min(1.0)
        };
        self.service = Some(Service {
            elapsed_s: 0.0,
            fuel_target_l: fuel_target,
            ve_target,
            change_tyres: request.change_tyres,
            compound: request.compound,
            from: None,
        });
        let tyres = if request.change_tyres {
            TYRE_SWAP_START_S + TYRE_SWAP_RAMP_S + TYRE_CHANGE_S
        } else {
            0.0
        };
        (added / REFUEL_RATE_LPS).max(tyres) + SERVICE_OVERHEAD_S
    }

    /// Ends the pit service (the car leaves the box).
    pub fn end_service(&mut self) {
        self.service = None;
    }

    /// Advances the player-car state by one tick.
    pub fn update(&mut self, vehicle: &VehicleProfile, input: &TickInput, rng: &mut ChaCha8Rng) {
        let dt = input.dt_s;
        let v = input.v_mps;
        self.lateral_m = self.excursion_lateral(input.half_width_m);
        self.update_driver(vehicle, input, rng);
        self.update_powertrain(vehicle, input);
        self.update_energy(vehicle, input);
        self.update_service(vehicle, input);
        let a_lat = v * v * input.point.curvature_per_m;
        self.update_tyres(vehicle, input, a_lat);
        self.update_brakes(vehicle, input);
        self.update_fluids(vehicle, input);
        self.update_chassis(vehicle, input, a_lat);
        self.update_body(input, a_lat, rng);
        self.excursion_left_s = (self.excursion_left_s - dt).max(0.0);
    }

    fn update_driver(&mut self, vehicle: &VehicleProfile, input: &TickInput, rng: &mut ChaCha8Rng) {
        let dt = input.dt_s;
        let v = input.v_mps;
        let (mut thr, brk) = if input.stopped {
            (0.0, 0.0)
        } else {
            pedals_for(vehicle, v, input.a_long_mps2, input.grip)
        };
        if thr > 0.0 && thr < 1.0 {
            thr = (thr + PEDAL_NOISE * (rng.random::<f64>() * 2.0 - 1.0)).clamp(0.0, 1.0);
        }
        self.throttle = approach(self.throttle, thr, dt, PEDAL_TAU_S);
        self.brake = approach(self.brake, brk, dt, PEDAL_TAU_S);

        self.tc_hold_s = (self.tc_hold_s - dt).max(0.0);
        self.abs_hold_s = (self.abs_hold_s - dt).max(0.0);
        let aid_p = AID_TRIGGER_PROBABILITY * (1.0 + input.wetness);
        if self.throttle > 0.85 && self.gear <= 3 && rng.random::<f64>() < aid_p {
            self.tc_hold_s = AID_HOLD_S;
        }
        if vehicle.abs_level_max > 0 && self.brake > 0.6 && rng.random::<f64>() < aid_p {
            self.abs_hold_s = AID_HOLD_S;
        }
        self.tc_active = self.tc_hold_s > 0.0;
        self.abs_active = self.abs_hold_s > 0.0;
        self.throttle_filtered = if self.tc_active {
            self.throttle * 0.85
        } else {
            self.throttle
        };
        self.brake_filtered = if self.abs_active {
            self.brake * 0.9
        } else {
            self.brake
        };

        // Road-wheel angle (left +) from the path curvature plus understeer, then wheel input (+ right).
        let kappa = input.point.curvature_per_m;
        let a_lat_g = v * v * kappa / STANDARD_GRAVITY;
        let road_wheel = (vehicle.wheelbase_m * kappa).atan() + UNDERSTEER_RAD_PER_G * a_lat_g;
        self.steering_noise = (self.steering_noise
            + STEERING_NOISE_STEP * (rng.random::<f64>() * 2.0 - 1.0))
            .clamp(-STEERING_NOISE_BOUND, STEERING_NOISE_BOUND);
        let half_lock = vehicle.steering_lock_rad / 2.0;
        self.steering = (-road_wheel * vehicle.steering_ratio / half_lock + self.steering_noise)
            .clamp(-1.0, 1.0);
        self.steering_torque_nm = -STEERING_TORQUE_PER_MPS2 * v * v * kappa;
    }

    fn update_powertrain(&mut self, vehicle: &VehicleProfile, input: &TickInput) {
        let v = input.v_mps;
        if input.stopped {
            self.gear = 0;
            self.clutch = 0.0;
            self.rpm = vehicle.idle_rpm;
        } else {
            self.gear = vehicle.select_gear(v, self.gear.max(1));
            let launch_rpm = vehicle.max_rpm * LAUNCH_RPM_SHARE;
            if v < LAUNCH_SPEED_MPS && self.gear == 1 && self.throttle > 0.1 {
                self.clutch = (1.0 - v / LAUNCH_SPEED_MPS).clamp(0.0, 1.0);
                self.rpm = vehicle.engine_rpm(v, 1).max(launch_rpm);
            } else {
                self.clutch = 0.0;
                self.rpm = vehicle.engine_rpm(v, self.gear);
            }
        }
        let omega = rpm_to_radps(self.rpm);
        self.engine_torque_nm = if self.throttle_filtered > 0.0 {
            self.throttle_filtered * vehicle.power_at_rpm(self.rpm) / omega.max(f64::EPSILON)
        } else if input.stopped {
            0.0
        } else {
            -ENGINE_DRAG_TORQUE_NM
        };
        self.limiter_on = input.in_pit_lane;
        self.limiter_active = self.limiter_on && v >= input.pit_speed_limit_mps - 0.5;
        self.turbo_boost_pa = if vehicle.turbo {
            TURBO_BASE_PA + TURBO_BOOST_PA * self.throttle_filtered * (self.rpm / vehicle.max_rpm)
        } else {
            0.0
        };
    }

    fn update_energy(&mut self, vehicle: &VehicleProfile, input: &TickInput) {
        let dt = input.dt_s;
        let weight = consumption_weight(self.throttle_filtered, self.rpm, vehicle.max_rpm) * dt;
        self.fuel_l = (self.fuel_l - self.fuel_k_per_weight * self.fuel_factor * weight).max(0.0);
        if vehicle.virtual_energy {
            self.virtual_energy =
                (self.virtual_energy - self.ve_k_per_weight * self.fuel_factor * weight).max(0.0);
        }
        if !vehicle.hybrid {
            return;
        }
        let v = input.v_mps;
        let deploy = if !self.limiter_on
            && self.throttle_filtered > DEPLOY_MIN_THROTTLE
            && v > vehicle.hybrid_min_deploy_speed_mps
            && self.battery_soc > SOC_MIN
        {
            vehicle.hybrid_deploy_power_w * self.throttle_filtered
        } else {
            0.0
        };
        let regen = if self.brake > 0.05 && self.battery_soc < SOC_MAX && v > LAUNCH_SPEED_MPS {
            REGEN_MAX_W * (self.brake * 2.0).min(1.0)
        } else {
            0.0
        };
        let capacity = vehicle.battery_capacity_j.max(f64::EPSILON);
        self.battery_soc = (self.battery_soc + (regen * REGEN_EFFICIENCY - deploy) / capacity * dt)
            .clamp(0.0, 1.0);
        self.regen_power_w = regen;
        self.motor_state = if deploy > 0.0 {
            HybridMotorState::Propulsion
        } else if regen > 0.0 {
            HybridMotorState::Regeneration
        } else {
            HybridMotorState::Inactive
        };
        let motor_omega = v / vehicle.tyre_radius_m * vehicle.hybrid_motor_ratio;
        self.motor_rpm = radps_to_rpm(motor_omega);
        self.motor_torque_nm = (deploy - regen) / motor_omega.max(1.0);
        let load = (deploy + regen) / vehicle.hybrid_deploy_power_w.max(f64::EPSILON);
        self.motor_temp_c = approach(
            self.motor_temp_c,
            MOTOR_TEMP_BASE_C + MOTOR_TEMP_RISE_C * load,
            dt,
            MOTOR_TEMP_TAU_S,
        );
        self.motor_water_temp_c = approach(
            self.motor_water_temp_c,
            MOTOR_WATER_BASE_C + MOTOR_WATER_RISE_C * load,
            dt,
            MOTOR_WATER_TAU_S,
        );
    }

    fn update_service(&mut self, vehicle: &VehicleProfile, input: &TickInput) {
        let Some(mut svc) = self.service else {
            return;
        };
        let dt = input.dt_s;
        svc.elapsed_s += dt;
        if svc.elapsed_s > SERVICE_OVERHEAD_S / 2.0 && self.fuel_l < svc.fuel_target_l {
            let step = REFUEL_RATE_LPS * dt;
            let fraction = (step / (svc.fuel_target_l - self.fuel_l)).min(1.0);
            self.virtual_energy += (svc.ve_target - self.virtual_energy).max(0.0) * fraction;
            self.fuel_l = (self.fuel_l + step).min(svc.fuel_target_l);
        }
        if svc.change_tyres && svc.elapsed_s >= TYRE_SWAP_START_S {
            let from = *svc.from.get_or_insert(TyreSnapshot {
                carcass_c: self.tyre_carcass_c,
                surface_c: self.tyre_surface_c,
                wear: self.tyre_wear,
            });
            if let Some(c) = svc.compound {
                self.compound = c;
            }
            let f = ((svc.elapsed_s - TYRE_SWAP_START_S) / TYRE_SWAP_RAMP_S).clamp(0.0, 1.0);
            let fresh = input.air_temp_c + FRESH_TYRE_ABOVE_AIR_C;
            for w in 0..WHEELS {
                self.tyre_carcass_c[w] = lerp(from.carcass_c[w], fresh, f);
                self.tyre_wear[w] = lerp(from.wear[w], 1.0, f);
                for z in 0..ZONES {
                    self.tyre_surface_c[w][z] = lerp(from.surface_c[w][z], fresh, f);
                }
            }
            self.tyre_cold_pressure_pa = vehicle.tyre_cold_pressure_pa;
            self.util_avg = [0.0; WHEELS];
        }
        self.service = Some(svc);
    }

    fn tyres_being_swapped(&self) -> bool {
        self.service
            .is_some_and(|s| s.change_tyres && s.elapsed_s >= TYRE_SWAP_START_S)
    }

    fn update_tyres(&mut self, vehicle: &VehicleProfile, input: &TickInput, a_lat: f64) {
        let dt = input.dt_s;
        let v = input.v_mps;
        let m = vehicle.mass_kg;
        let g = STANDARD_GRAVITY;
        let df = vehicle.downforce_n(v);
        let df_front = df * vehicle.front_downforce_share;
        let long_transfer = m * input.a_long_mps2 * vehicle.cg_height_m / vehicle.wheelbase_m;
        let front_axle = m * g * vehicle.front_weight_share + df_front - long_transfer;
        let rear_axle =
            m * g * (1.0 - vehicle.front_weight_share) + (df - df_front) + long_transfer;
        let total = (front_axle + rear_axle).max(f64::EPSILON);
        let lat_transfer = m * a_lat * vehicle.cg_height_m / vehicle.track_width_m;
        let lt_front = lat_transfer * front_axle / total;
        let lt_rear = lat_transfer - lt_front;
        // A left turn (a_lat > 0) loads the right-hand wheels.
        self.tyre_load_n = [
            (front_axle / 2.0 - lt_front / 2.0).max(0.0),
            (front_axle / 2.0 + lt_front / 2.0).max(0.0),
            (rear_axle / 2.0 - lt_rear / 2.0).max(0.0),
            (rear_axle / 2.0 + lt_rear / 2.0).max(0.0),
        ];
        let load_sum: f64 = self.tyre_load_n.iter().sum::<f64>().max(f64::EPSILON);
        let drive_force = m * (input.a_long_mps2 + vehicle.drag_decel(v) + ROLLING_DECEL_MPS2);
        let mu = vehicle.grip_mu * input.grip;
        for w in 0..WHEELS {
            let load = self.tyre_load_n[w];
            let rear = w >= 2;
            self.tyre_lat_force_n[w] = m * a_lat * load / load_sum;
            self.tyre_long_force_n[w] = if drive_force >= 0.0 {
                if rear { drive_force / 2.0 } else { 0.0 }
            } else {
                let share = if rear {
                    vehicle.brake_bias_rear
                } else {
                    1.0 - vehicle.brake_bias_rear
                };
                drive_force * share / 2.0
            };
            let force = self.tyre_lat_force_n[w].hypot(self.tyre_long_force_n[w]);
            let util = (force / (mu * load).max(f64::EPSILON)).clamp(0.0, UTIL_MAX);
            self.tyre_sliding[w] = ((util - SLIDING_ONSET) / SLIDING_SPAN).clamp(0.0, 1.0);
            self.util_avg[w] = approach(self.util_avg[w], util, dt, UTIL_AVG_TAU_S);
            if self.tyres_being_swapped() {
                continue;
            }
            let carcass_eq = input.track_temp_c
                + CARCASS_SPEED_RISE_C * (v / CARCASS_SPEED_REF_MPS).min(1.5)
                + CARCASS_WORK_RISE_C * self.util_avg[w]
                - CARCASS_WET_COOLING_C * input.wetness;
            self.tyre_carcass_c[w] =
                approach(self.tyre_carcass_c[w], carcass_eq, dt, CARCASS_TAU_S);
            let left_side_wheel = w % 2 == 0;
            // Zone order L, M, R; the inner edge is on the car's centre-line side.
            let camber = if left_side_wheel {
                [CAMBER_OUTER_C, CAMBER_MIDDLE_C, CAMBER_INNER_C]
            } else {
                [CAMBER_INNER_C, CAMBER_MIDDLE_C, CAMBER_OUTER_C]
            };
            // In a left turn the right edge of every tyre is the loaded outside edge.
            let edge = SURFACE_CORNER_EDGE_C * (a_lat / (2.0 * g)).abs().min(1.0);
            let edge_zone = if a_lat >= 0.0 { 2 } else { 0 };
            let carcass = self.tyre_carcass_c[w];
            for (z, (surface, camber_c)) in
                self.tyre_surface_c[w].iter_mut().zip(camber).enumerate()
            {
                let extra = if z == edge_zone { edge } else { 0.0 };
                let target = carcass + SURFACE_WORK_RISE_C * util + camber_c + extra;
                *surface = approach(*surface, target, dt, SURFACE_TAU_S);
            }
            let wear = WEAR_PER_M
                * (WEAR_BASE + util * util)
                * input.distance_m
                * compound_wear_factor(self.compound);
            self.tyre_wear[w] = (self.tyre_wear[w] - wear).clamp(0.0, 1.0);
        }
    }

    fn update_brakes(&mut self, vehicle: &VehicleProfile, input: &TickInput) {
        let dt = input.dt_s;
        let v = input.v_mps;
        let rear_ratio = vehicle.brake_bias_rear / (1.0 - vehicle.brake_bias_rear);
        let front = self.brake_filtered;
        let rear = (self.brake_filtered * rear_ratio).min(1.0);
        self.brake_pressure = [front, front, rear, rear];
        for w in 0..WHEELS {
            let heat_share = if w >= 2 { REAR_BRAKE_HEAT_SHARE } else { 1.0 };
            let heat = BRAKE_HEAT_PER_MPS * self.brake_pressure[w] * v * heat_share;
            let cool = (self.brake_temp_c[w] - input.air_temp_c)
                * (BRAKE_COOL_BASE + BRAKE_COOL_PER_MPS * v);
            self.brake_temp_c[w] += (heat - cool) * dt;
        }
    }

    fn update_fluids(&mut self, vehicle: &VehicleProfile, input: &TickInput) {
        let dt = input.dt_s;
        self.throttle_avg = approach(self.throttle_avg, self.throttle, dt, THROTTLE_AVG_TAU_S);
        let ambient = FLUID_AMBIENT_GAIN * (input.air_temp_c - FLUID_AMBIENT_REF_C)
            - FLUID_WET_COOLING_C * input.wetness
            + if input.stopped {
                FLUID_STATIONARY_RISE_C
            } else {
                0.0
            };
        let load = self.throttle_avg - 0.5;
        self.water_temp_c = approach(
            self.water_temp_c,
            vehicle.water_temp_nominal_c + WATER_LOAD_RISE_C * load + ambient,
            dt,
            FLUID_TAU_S,
        );
        self.oil_temp_c = approach(
            self.oil_temp_c,
            vehicle.oil_temp_nominal_c + OIL_LOAD_RISE_C * load + ambient,
            dt,
            FLUID_TAU_S,
        );
    }

    fn update_chassis(&mut self, vehicle: &VehicleProfile, input: &TickInput, a_lat: f64) {
        let v = input.v_mps;
        let df = vehicle.downforce_n(v);
        self.downforce_front_n = df * vehicle.front_downforce_share;
        self.downforce_rear_n = df - self.downforce_front_n;
        self.drag_n = vehicle.drag_force_n(v);
        let pitch = RIDE_PITCH_M_PER_MPS2 * input.a_long_mps2;
        self.ride_front_m = (RIDE_FRONT_STATIC_M - RIDE_AERO_M_PER_N * self.downforce_front_n
            + pitch)
            .clamp(RIDE_MIN_M, RIDE_MAX_M);
        self.ride_rear_m = (RIDE_REAR_STATIC_M - RIDE_AERO_M_PER_N * self.downforce_rear_n - pitch)
            .clamp(RIDE_MIN_M, RIDE_MAX_M);
        // A left turn rolls the body onto the right-hand wheels.
        let roll = RIDE_ROLL_M_PER_MPS2 * a_lat;
        self.ride_height_m = [
            (self.ride_front_m + roll).clamp(RIDE_MIN_M, RIDE_MAX_M),
            (self.ride_front_m - roll).clamp(RIDE_MIN_M, RIDE_MAX_M),
            (self.ride_rear_m + roll).clamp(RIDE_MIN_M, RIDE_MAX_M),
            (self.ride_rear_m - roll).clamp(RIDE_MIN_M, RIDE_MAX_M),
        ];
        for w in 0..WHEELS {
            self.susp_deflection_m[w] = self.tyre_load_n[w] / WHEEL_RATE_N_PER_M;
        }
        let kappa = input.point.curvature_per_m;
        let half_track = vehicle.track_width_m / 2.0;
        for w in 0..WHEELS {
            // Inner wheels of a left turn (left side) roll slower.
            let side = if w % 2 == 0 { -1.0 } else { 1.0 };
            let mut wheel_v = v * (1.0 + side * kappa * half_track);
            if w >= 2 && self.gear >= 1 && self.gear <= TRACTION_SLIP_MAX_GEAR {
                wheel_v *= 1.0 + TRACTION_SLIP * self.throttle;
            }
            if w < 2 && self.abs_active {
                wheel_v *= 1.0 - ABS_LOCK_SLIP * self.brake;
            }
            self.wheel_rot_radps[w] = wheel_v.max(0.0) / vehicle.tyre_radius_m;
        }
        let left_wheels_off = -self.lateral_m + half_track > input.half_width_m;
        for w in 0..WHEELS {
            let left_side_wheel = w % 2 == 0;
            self.surface[w] = if left_wheels_off && left_side_wheel {
                SurfaceType::Grass
            } else if input.wetness > WET_SURFACE_WETNESS {
                SurfaceType::Wet
            } else {
                SurfaceType::Dry
            };
        }
    }

    fn update_body(&mut self, input: &TickInput, a_lat: f64, rng: &mut ChaCha8Rng) {
        let dt = input.dt_s;
        let v = input.v_mps;
        let p = &input.point;
        let moving = v > 0.0;
        let noise =
            |rng: &mut ChaCha8Rng, sigma: f64| if moving { sigma * gaussian(rng) } else { 0.0 };
        // Velocity: small body slip to the outside of the corner, vertical from the slope, the rest forward.
        let vx = -BODY_SLIP_PER_G * v * a_lat / STANDARD_GRAVITY;
        let vy = v * p.slope;
        let vz = -(v * v - vx * vx - vy * vy).max(0.0).sqrt();
        self.local_vel = [vx, vy, vz];
        let ax = a_lat + noise(rng, ACCEL_NOISE_LAT);
        let ay = v * v * p.vertical_curvature_per_m + noise(rng, ACCEL_NOISE_VERT);
        let az = -input.a_long_mps2 + noise(rng, ACCEL_NOISE_LONG);
        self.local_accel = [ax, ay, az];
        // Yaw left = v·κ; LMU +rot.y yaws right.
        let rot = [
            noise(rng, ROT_NOISE),
            -v * p.curvature_per_m + noise(rng, ROT_NOISE),
            noise(rng, ROT_NOISE),
        ];
        for (k, value) in rot.iter().enumerate() {
            self.local_rot_accel[k] = (value - self.local_rot[k]) / dt;
        }
        self.local_rot = rot;
    }

    /// Lateral offset during a track-limits excursion: out beyond the left edge and back along a half
    /// sine, so the position changes smoothly.
    fn excursion_lateral(&self, half_width_m: f64) -> f64 {
        if self.excursion_left_s <= 0.0 {
            return 0.0;
        }
        let progress = 1.0 - self.excursion_left_s / EXCURSION_S;
        -(half_width_m + EXCURSION_BEYOND_EDGE_M) * (std::f64::consts::PI * progress).sin()
    }
}

fn lerp(a: f64, b: f64, f: f64) -> f64 {
    a + (b - a) * f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::TrackSpec;
    use crate::vehicle::VehicleKind;

    #[test]
    fn pedals_invert_the_acceleration_model() {
        let v = VehicleProfile::new(VehicleKind::HybridHypercar);
        let (t, b) = pedals_for(&v, 50.0, 2.0, 1.0);
        assert!(t > 0.0 && t <= 1.0 && b == 0.0);
        let (t, b) = pedals_for(&v, 70.0, -30.0, 1.0);
        assert!(t == 0.0 && b > 0.5 && b <= 1.0);
        let (t, b) = pedals_for(&v, 20.0, -0.5, 1.0);
        assert_eq!((t, b), (0.0, 0.0), "coasting");
    }

    #[test]
    fn lap_weight_is_positive() {
        let track = Track::build(&TrackSpec::synthetic_circuit()).expect("track");
        let v = VehicleProfile::new(VehicleKind::HybridHypercar);
        let profile = SpeedProfile::compute(&track, &v, 1.0);
        let w = lap_consumption_weight(&track, &v, &profile);
        // Between idle-only and full throttle at max rpm for the whole lap.
        assert!(
            w > IDLE_FUEL_SHARE * profile.lap_time_s() && w < profile.lap_time_s(),
            "{w}"
        );
    }
}

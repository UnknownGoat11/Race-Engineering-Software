//! Synthetic vehicle profiles and the quasi-steady-state physics shared by the speed profile and the
//! per-tick simulation (point mass with aero, friction limit growing with downforce).

use std::f64::consts::TAU;

use telemetry_model::enums::{CompoundType, VehicleChampionship, VehicleClass};
use telemetry_model::units::STANDARD_GRAVITY;

use crate::mode::synthetic_name;

/// Air density used for drag (kg/m³).
pub const AIR_DENSITY_KG_M3: f64 = 1.2;
/// Speed below which power-limited acceleration is evaluated at this speed (avoids P/v → ∞) (m/s).
const MIN_POWER_SPEED_MPS: f64 = 5.0;
/// Share of the friction limit available for braking.
const BRAKE_GRIP_SHARE: f64 = 0.9;
/// Rolling resistance as a deceleration (m/s²).
pub const ROLLING_DECEL_MPS2: f64 = 0.15;
/// Engine braking with the throttle closed (m/s²).
pub const ENGINE_BRAKE_DECEL_MPS2: f64 = 1.0;
/// Seconds per minute (rpm ↔ rad/s).
const SECONDS_PER_MINUTE: f64 = 60.0;

/// Which built-in vehicle to simulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VehicleKind {
    /// Hybrid Hypercar: front e-motor, battery, turbo, virtual energy.
    HybridHypercar,
    /// LMGT3: no hybrid system (every hybrid channel NOT_SUPPORTED), naturally aspirated, virtual energy.
    Gt3,
}

/// Static parameters of a simulated car. Values are plausible for the class, not copies of a real car.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleProfile {
    pub kind: VehicleKind,
    /// Identity strings (all carry the synthetic prefix).
    pub name: String,
    pub model: String,
    pub class_name: String,
    pub filename: String,
    pub class: VehicleClass,
    pub championship: VehicleChampionship,
    pub hybrid: bool,
    pub virtual_energy: bool,
    pub turbo: bool,
    pub mass_kg: f64,
    pub peak_power_w: f64,
    /// Drag coefficient × frontal area (m²).
    pub drag_area_m2: f64,
    /// Tyre friction coefficient (dry, new).
    pub grip_mu: f64,
    /// Downforce per unit mass and speed² (1/m): downforce = m · c · v².
    pub downforce_coeff_per_m: f64,
    pub front_downforce_share: f64,
    /// Share of the friction limit usable for traction.
    pub traction_share: f64,
    pub max_speed_mps: f64,
    pub idle_rpm: f64,
    pub max_rpm: f64,
    pub shift_rpm: f64,
    pub downshift_rpm: f64,
    /// Overall ratios (gearbox × final drive), first gear first.
    pub gear_ratios: Vec<f64>,
    pub tyre_radius_m: f64,
    pub wheelbase_m: f64,
    pub track_width_m: f64,
    pub cg_height_m: f64,
    pub front_weight_share: f64,
    pub steering_ratio: f64,
    /// Lock-to-lock steering wheel range (rad).
    pub steering_lock_rad: f64,
    pub fuel_capacity_l: f64,
    /// Nominal fuel per lap of the default circuit (L).
    pub fuel_per_lap_l: f64,
    /// Nominal virtual energy per lap of the default circuit (fraction).
    pub ve_per_lap_frac: f64,
    pub brake_bias_rear: f64,
    pub tyre_optimal_temp_c: f64,
    pub tyre_cold_pressure_pa: f64,
    pub default_compound: CompoundType,
    pub hybrid_deploy_power_w: f64,
    pub hybrid_min_deploy_speed_mps: f64,
    pub battery_capacity_j: f64,
    pub hybrid_motor_ratio: f64,
    pub tc_level: u8,
    pub tc_level_max: u8,
    pub abs_level: u8,
    pub abs_level_max: u8,
    pub water_temp_nominal_c: f64,
    pub oil_temp_nominal_c: f64,
}

impl VehicleProfile {
    /// Built-in profile for `kind`.
    pub fn new(kind: VehicleKind) -> Self {
        match kind {
            VehicleKind::HybridHypercar => Self::hypercar(),
            VehicleKind::Gt3 => Self::gt3(),
        }
    }

    fn hypercar() -> Self {
        Self {
            kind: VehicleKind::HybridHypercar,
            name: synthetic_name("Hypercar H1 #7"),
            model: synthetic_name("Hypercar H1"),
            class_name: synthetic_name("Hypercar"),
            filename: synthetic_name("SYN_H1.VEH"),
            class: VehicleClass::Hypercar,
            championship: VehicleChampionship::Wec2025,
            hybrid: true,
            virtual_energy: true,
            turbo: true,
            mass_kg: 1_100.0,
            peak_power_w: 500_000.0,
            drag_area_m2: 1.1,
            grip_mu: 1.6,
            downforce_coeff_per_m: 2.0e-3,
            front_downforce_share: 0.45,
            traction_share: 0.6,
            max_speed_mps: 92.0,
            idle_rpm: 2_500.0,
            max_rpm: 9_000.0,
            shift_rpm: 8_700.0,
            downshift_rpm: 5_200.0,
            gear_ratios: geometric_ratios(11.0, 3.6, 7),
            tyre_radius_m: 0.36,
            wheelbase_m: 3.15,
            track_width_m: 1.6,
            cg_height_m: 0.30,
            front_weight_share: 0.47,
            steering_ratio: 11.0,
            steering_lock_rad: 540.0_f64.to_radians(),
            fuel_capacity_l: 90.0,
            fuel_per_lap_l: 2.9,
            ve_per_lap_frac: 0.055,
            brake_bias_rear: 0.44,
            tyre_optimal_temp_c: 85.0,
            tyre_cold_pressure_pa: 140_000.0,
            default_compound: CompoundType::Medium,
            hybrid_deploy_power_w: 200_000.0,
            hybrid_min_deploy_speed_mps: 190.0 / 3.6,
            battery_capacity_j: 3.0e6,
            hybrid_motor_ratio: 9.0,
            tc_level: 4,
            tc_level_max: 11,
            abs_level: 0,
            abs_level_max: 0,
            water_temp_nominal_c: 88.0,
            oil_temp_nominal_c: 105.0,
        }
    }

    fn gt3() -> Self {
        Self {
            kind: VehicleKind::Gt3,
            name: synthetic_name("GT3 G3 #77"),
            model: synthetic_name("GT3 G3"),
            class_name: synthetic_name("LMGT3"),
            filename: synthetic_name("SYN_G3.VEH"),
            class: VehicleClass::Gt3,
            championship: VehicleChampionship::Wec2025,
            hybrid: false,
            virtual_energy: true,
            turbo: false,
            mass_kg: 1_350.0,
            peak_power_w: 390_000.0,
            drag_area_m2: 1.0,
            grip_mu: 1.5,
            downforce_coeff_per_m: 1.1e-3,
            front_downforce_share: 0.42,
            traction_share: 0.6,
            max_speed_mps: 78.0,
            idle_rpm: 1_800.0,
            max_rpm: 8_000.0,
            shift_rpm: 7_600.0,
            downshift_rpm: 4_600.0,
            gear_ratios: geometric_ratios(10.5, 3.57, 6),
            tyre_radius_m: 0.35,
            wheelbase_m: 2.75,
            track_width_m: 1.65,
            cg_height_m: 0.38,
            front_weight_share: 0.46,
            steering_ratio: 13.0,
            steering_lock_rad: 720.0_f64.to_radians(),
            fuel_capacity_l: 110.0,
            fuel_per_lap_l: 3.4,
            ve_per_lap_frac: 0.06,
            brake_bias_rear: 0.42,
            tyre_optimal_temp_c: 90.0,
            tyre_cold_pressure_pa: 150_000.0,
            default_compound: CompoundType::Medium,
            hybrid_deploy_power_w: 0.0,
            hybrid_min_deploy_speed_mps: 0.0,
            battery_capacity_j: 0.0,
            hybrid_motor_ratio: 0.0,
            tc_level: 3,
            tc_level_max: 12,
            abs_level: 4,
            abs_level_max: 12,
            water_temp_nominal_c: 90.0,
            oil_temp_nominal_c: 110.0,
        }
    }

    /// Number of forward gears.
    pub fn gear_count(&self) -> usize {
        self.gear_ratios.len()
    }

    /// Aerodynamic drag as a deceleration (m/s²).
    pub fn drag_decel(&self, v_mps: f64) -> f64 {
        0.5 * AIR_DENSITY_KG_M3 * self.drag_area_m2 * v_mps * v_mps / self.mass_kg
    }

    /// Aerodynamic drag force (N).
    pub fn drag_force_n(&self, v_mps: f64) -> f64 {
        self.drag_decel(v_mps) * self.mass_kg
    }

    /// Total downforce (N).
    pub fn downforce_n(&self, v_mps: f64) -> f64 {
        self.mass_kg * self.downforce_coeff_per_m * v_mps * v_mps
    }

    /// Friction-limited acceleration magnitude at speed `v` with grip multiplier `grip` (m/s²).
    pub fn grip_accel(&self, v_mps: f64, grip: f64) -> f64 {
        self.grip_mu * grip * (STANDARD_GRAVITY + self.downforce_coeff_per_m * v_mps * v_mps)
    }

    /// Largest propulsive acceleration (before drag and rolling resistance) (m/s²).
    pub fn max_drive_accel(&self, v_mps: f64, grip: f64) -> f64 {
        let traction = self.traction_share * self.grip_accel(v_mps, grip);
        let power = self.peak_power_w / (self.mass_kg * v_mps.max(MIN_POWER_SPEED_MPS));
        traction.min(power)
    }

    /// Largest braking deceleration from the brakes alone (m/s²).
    pub fn max_brake_decel(&self, v_mps: f64, grip: f64) -> f64 {
        BRAKE_GRIP_SHARE * self.grip_accel(v_mps, grip)
    }

    /// Highest speed through a corner of curvature `kappa` (m/s), capped at the top speed.
    pub fn corner_speed_limit(&self, kappa_per_m: f64, grip: f64) -> f64 {
        let mu = self.grip_mu * grip;
        let den = kappa_per_m.abs() - mu * self.downforce_coeff_per_m;
        if den <= f64::EPSILON {
            self.max_speed_mps
        } else {
            (mu * STANDARD_GRAVITY / den).sqrt().min(self.max_speed_mps)
        }
    }

    /// Engine speed at road speed `v` in `gear` (1-based; 0 = neutral → idle) (rpm), at least idle.
    pub fn engine_rpm(&self, v_mps: f64, gear: i32) -> f64 {
        let ratio = usize::try_from(gear)
            .ok()
            .and_then(|g| g.checked_sub(1))
            .and_then(|g| self.gear_ratios.get(g));
        match ratio {
            Some(r) => (v_mps / self.tyre_radius_m * r * SECONDS_PER_MINUTE / TAU)
                .clamp(self.idle_rpm, self.max_rpm),
            None => self.idle_rpm,
        }
    }

    /// Gear after a shift decision at speed `v` (hysteresis between up- and downshift points).
    pub fn select_gear(&self, v_mps: f64, current: i32) -> i32 {
        let top = i32::try_from(self.gear_count()).unwrap_or(1);
        let mut gear = current.clamp(1, top);
        while gear < top && self.raw_rpm(v_mps, gear) > self.shift_rpm {
            gear += 1;
        }
        while gear > 1 && self.raw_rpm(v_mps, gear) < self.downshift_rpm {
            // Only downshift if the lower gear stays below the shift point.
            if self.raw_rpm(v_mps, gear - 1) >= self.shift_rpm {
                break;
            }
            gear -= 1;
        }
        gear
    }

    fn raw_rpm(&self, v_mps: f64, gear: i32) -> f64 {
        let ratio = usize::try_from(gear - 1)
            .ok()
            .and_then(|g| self.gear_ratios.get(g))
            .copied()
            .unwrap_or(0.0);
        v_mps / self.tyre_radius_m * ratio * SECONDS_PER_MINUTE / TAU
    }

    /// Engine power available at `rpm` with full throttle (W): a simple rising power curve.
    pub fn power_at_rpm(&self, rpm: f64) -> f64 {
        const POWER_AT_IDLE_SHARE: f64 = 0.45;
        let x = (rpm / self.max_rpm).clamp(0.0, 1.0);
        self.peak_power_w * (POWER_AT_IDLE_SHARE + (1.0 - POWER_AT_IDLE_SHARE) * x)
    }
}

/// `count` ratios from `first` to `last`, evenly spaced on a log scale.
fn geometric_ratios(first: f64, last: f64, count: usize) -> Vec<f64> {
    let steps = count.saturating_sub(1).max(1) as f64;
    let q = (last / first).powf(1.0 / steps);
    (0..count).map(|i| first * q.powi(i as i32)).collect()
}

/// rpm → rad/s.
pub fn rpm_to_radps(rpm: f64) -> f64 {
    rpm * TAU / SECONDS_PER_MINUTE
}

/// rad/s → rpm.
pub fn radps_to_rpm(radps: f64) -> f64 {
    radps * SECONDS_PER_MINUTE / TAU
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gears_cover_the_speed_range() {
        for kind in [VehicleKind::HybridHypercar, VehicleKind::Gt3] {
            let v = VehicleProfile::new(kind);
            let top = i32::try_from(v.gear_count()).expect("gears");
            assert_eq!(v.select_gear(v.max_speed_mps, 1), top);
            assert_eq!(v.select_gear(10.0, top), 1);
            assert!(v.engine_rpm(v.max_speed_mps, top) <= v.max_rpm);
            assert!(v.engine_rpm(v.max_speed_mps, top) > v.shift_rpm * 0.8);
            assert_eq!(v.engine_rpm(0.0, 0), v.idle_rpm);
            // Hysteresis: holding a gear between the shift points.
            let g = v.select_gear(40.0, 3);
            assert_eq!(v.select_gear(40.0, g), g);
            assert!(v.name.starts_with(crate::mode::SYNTHETIC_NAME_PREFIX));
        }
    }

    #[test]
    fn physics_limits_are_plausible() {
        let v = VehicleProfile::new(VehicleKind::HybridHypercar);
        let g = STANDARD_GRAVITY;
        // ~1.6 g at low speed, > 3 g at 70 m/s thanks to downforce.
        assert!((v.grip_accel(0.0, 1.0) / g - 1.6).abs() < 1e-9);
        assert!(v.grip_accel(70.0, 1.0) / g > 3.0);
        // Power limited at high speed, drag balances near the top speed.
        assert!(v.max_drive_accel(90.0, 1.0) < v.drag_decel(90.0) + 2.0);
        assert!(v.corner_speed_limit(1.0 / 35.0, 1.0) < 30.0);
        assert_eq!(v.corner_speed_limit(0.0, 1.0), v.max_speed_mps);
        assert!((radps_to_rpm(rpm_to_radps(6_000.0)) - 6_000.0).abs() < 1e-9);
    }
}

//! Quasi-steady-state racing-line speed profile: corner limits from the friction circle with downforce,
//! then forward (traction/power) and backward (braking) passes around the closed lap.

use crate::track::Track;
use crate::vehicle::{ROLLING_DECEL_MPS2, VehicleProfile};

/// Forward/backward sweeps; three are enough for the closed-loop limits to converge.
const PROFILE_SWEEPS: usize = 3;

/// Target speed per track station for one vehicle and grip level.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeedProfile {
    speeds_mps: Vec<f64>,
    spacing_m: f64,
    lap_time_s: f64,
}

impl SpeedProfile {
    /// Computes the profile of `vehicle` on `track` with grip multiplier `grip` (1.0 = dry).
    pub fn compute(track: &Track, vehicle: &VehicleProfile, grip: f64) -> Self {
        let n = track.station_count();
        let ds = track.spacing_m();
        let mut v: Vec<f64> = (0..n)
            .map(|i| vehicle.corner_speed_limit(track.station_curvature(i), grip))
            .collect();
        for _ in 0..PROFILE_SWEEPS {
            for i in 0..n {
                let j = (i + 1) % n;
                let net = vehicle.max_drive_accel(v[i], grip)
                    - vehicle.drag_decel(v[i])
                    - ROLLING_DECEL_MPS2;
                let reachable = (v[i] * v[i] + 2.0 * net * ds).max(0.0).sqrt();
                v[j] = v[j].min(reachable);
            }
            for i in (0..n).rev() {
                let j = (i + 1) % n;
                let decel = vehicle.max_brake_decel(v[j], grip)
                    + vehicle.drag_decel(v[j])
                    + ROLLING_DECEL_MPS2;
                let reachable = (v[j] * v[j] + 2.0 * decel * ds).sqrt();
                v[i] = v[i].min(reachable);
            }
        }
        let lap_time_s = (0..n)
            .map(|i| 2.0 * ds / (v[i] + v[(i + 1) % n]).max(f64::EPSILON))
            .sum();
        Self {
            speeds_mps: v,
            spacing_m: ds,
            lap_time_s,
        }
    }

    /// Target speed at lap distance `s` (linear interpolation, wraps).
    pub fn speed_at(&self, s_m: f64) -> f64 {
        let n = self.speeds_mps.len();
        if n == 0 {
            return 0.0;
        }
        let length = n as f64 * self.spacing_m;
        let pos = s_m.rem_euclid(length) / self.spacing_m;
        let i = (pos.floor() as usize) % n;
        let f = pos - pos.floor();
        self.speeds_mps[i] + (self.speeds_mps[(i + 1) % n] - self.speeds_mps[i]) * f
    }

    /// Ideal lap time of this profile (s).
    pub fn lap_time_s(&self) -> f64 {
        self.lap_time_s
    }

    /// Lowest and highest target speed (m/s).
    pub fn speed_range_mps(&self) -> (f64, f64) {
        self.speeds_mps
            .iter()
            .fold((f64::INFINITY, 0.0_f64), |(lo, hi), &v| {
                (lo.min(v), hi.max(v))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::TrackSpec;
    use crate::vehicle::VehicleKind;

    #[test]
    fn lap_times_are_plausible() {
        let track = Track::build(&TrackSpec::synthetic_circuit()).expect("track");
        let hyper = VehicleProfile::new(VehicleKind::HybridHypercar);
        let gt3 = VehicleProfile::new(VehicleKind::Gt3);
        let dry = SpeedProfile::compute(&track, &hyper, 1.0);
        let wet = SpeedProfile::compute(&track, &hyper, 0.72);
        let gt = SpeedProfile::compute(&track, &gt3, 1.0);
        assert!(
            (75.0..95.0).contains(&dry.lap_time_s()),
            "{}",
            dry.lap_time_s()
        );
        assert!(wet.lap_time_s() > dry.lap_time_s() + 5.0);
        assert!(gt.lap_time_s() > dry.lap_time_s() + 5.0);
        let (lo, hi) = dry.speed_range_mps();
        assert!(lo > 15.0 && lo < 35.0, "{lo}");
        assert!(hi <= hyper.max_speed_mps && hi > 80.0, "{hi}");
        assert!((dry.speed_at(track.length_m()) - dry.speed_at(0.0)).abs() < 1e-9);
    }
}

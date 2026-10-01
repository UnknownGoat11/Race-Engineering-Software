//! LMU local vehicle frame → canonical body frame (ISO 8855). ASSUMPTION A-LMU-024, pending Gate-1.
//!
//! LMU's local frame, as stated by the header: **+x out of the car's left side, +y up, +z towards the
//! rear**; rotation senses **+x pitches up, +y yaws right, +z rolls right**. The header's own remark about
//! ISO coordinates is internally inconsistent and is not used (research §11.13).
//!
//! Our body frame is ISO 8855 (right-handed: X forward, Y left, Z up):
//! `X_iso = −z_lmu`, `Y_iso = +x_lmu`, `Z_iso = +y_lmu`.
//!
//! Published sign conventions (docs/LMU_CHANNEL_MAP.md §18):
//! - **positive `dynamics.g_long` = acceleration** (throttle), negative = braking;
//! - **positive `dynamics.g_lat` = acceleration towards the car's LEFT**, i.e. a left-hand corner. A UI that
//!   wants "right = +" flips it for display only;
//! - positive `dynamics.g_vert` = upward acceleration (gravity inclusion UNVERIFIED, A-LMU-032);
//! - positive `dynamics.yaw_rate_radps` = yawing left (counter-clockwise seen from above);
//! - positive `dynamics.pitch_rate_radps` = nose down; positive `dynamics.roll_rate_radps` = rolling right;
//! - positive `vehicle.speed_forward_mps` = moving forward (LMU forward motion is −z).
//!
//! Every sign factor lives in this module as a named constant. If the Gate-1 check in a known right-hand
//! corner (expect `mLocalAccel.x < 0`, `mLocalRot.y > 0`, `mLocalVel.z < 0`) refutes A-LMU-024, only these
//! constants change.

use lmu_reader::Vec3;
use telemetry_model::units::convert;

/// ISO longitudinal (X, forward) component = this factor × LMU `z` (LMU +z points to the rear).
pub const ISO_X_FROM_LMU_Z: f64 = -1.0;
/// ISO lateral (Y, left) component = this factor × LMU `x` (LMU +x points out of the car's left side).
pub const ISO_Y_FROM_LMU_X: f64 = 1.0;
/// ISO vertical (Z, up) component = this factor × LMU `y` (LMU +y points up).
pub const ISO_Z_FROM_LMU_Y: f64 = 1.0;
/// ISO yaw rate (about Z, + = left) = this factor × LMU `rot.y` (LMU +y yaws right).
pub const ISO_YAW_FROM_LMU_ROT_Y: f64 = -1.0;
/// ISO pitch rate (about Y, + = nose down) = this factor × LMU `rot.x` (LMU +x pitches up).
pub const ISO_PITCH_FROM_LMU_ROT_X: f64 = -1.0;
/// ISO roll rate (about X, + = roll right) = this factor × LMU `rot.z` (LMU +z rolls right).
pub const ISO_ROLL_FROM_LMU_ROT_Z: f64 = 1.0;

/// `vehicle.speed_mps`: magnitude of `mLocalVel` (frame independent).
pub fn speed_mps(local_vel: Vec3) -> f64 {
    convert::magnitude(local_vel.x, local_vel.y, local_vel.z)
}

/// `vehicle.speed_forward_mps`: forward component of `mLocalVel`; negative when reversing.
pub fn forward_speed_mps(local_vel: Vec3) -> f64 {
    ISO_X_FROM_LMU_Z * local_vel.z
}

/// `dynamics.g_long`: + = accelerating, − = braking.
pub fn g_long(local_accel: Vec3) -> f64 {
    convert::accel_to_g(ISO_X_FROM_LMU_Z * local_accel.z)
}

/// `dynamics.g_lat`: + = acceleration towards the car's left (left-hand corner).
pub fn g_lat(local_accel: Vec3) -> f64 {
    convert::accel_to_g(ISO_Y_FROM_LMU_X * local_accel.x)
}

/// `dynamics.g_vert`: + = upward acceleration.
pub fn g_vert(local_accel: Vec3) -> f64 {
    convert::accel_to_g(ISO_Z_FROM_LMU_Y * local_accel.y)
}

/// `dynamics.g_combined`: friction-circle magnitude of lateral and longitudinal G.
pub fn g_combined(g_lat: f64, g_long: f64) -> f64 {
    g_lat.hypot(g_long)
}

/// `dynamics.yaw_rate_radps`: + = yawing left.
pub fn yaw_rate_radps(local_rot: Vec3) -> f64 {
    ISO_YAW_FROM_LMU_ROT_Y * local_rot.y
}

/// `dynamics.pitch_rate_radps`: + = nose down.
pub fn pitch_rate_radps(local_rot: Vec3) -> f64 {
    ISO_PITCH_FROM_LMU_ROT_X * local_rot.x
}

/// `dynamics.roll_rate_radps`: + = rolling right.
pub fn roll_rate_radps(local_rot: Vec3) -> f64 {
    ISO_ROLL_FROM_LMU_ROT_Z * local_rot.z
}

/// `vehicle.heading_rad` (A-LMU-025): `mOri` rows map local vectors to world components, so the world
/// forward vector is `−(mOri[0].z, mOri[1].z, mOri[2].z)` (local forward is −z). Heading is the angle of
/// that vector in the world x/z plane, `atan2(f.x, f.z)`. `None` when the forward vector is vertical or
/// degenerate (no heading exists).
pub fn heading_rad(ori: &[Vec3; 3]) -> Option<f64> {
    let forward_x = -ori[0].z;
    let forward_z = -ori[2].z;
    let degenerate = forward_x == 0.0 && forward_z == 0.0;
    (!degenerate && forward_x.is_finite() && forward_z.is_finite())
        .then(|| forward_x.atan2(forward_z))
}

#[cfg(test)]
mod tests {
    use super::*;
    use telemetry_model::units::STANDARD_GRAVITY;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x, y, z }
    }

    const EPS: f64 = 1e-12;

    #[test]
    fn speed_is_the_magnitude() {
        assert!((speed_mps(v(3.0, 0.0, -4.0)) - 5.0).abs() < EPS);
    }

    #[test]
    fn forward_motion_is_negative_lmu_z() {
        assert!((forward_speed_mps(v(0.0, 0.0, -50.0)) - 50.0).abs() < EPS);
        assert!(forward_speed_mps(v(0.0, 0.0, 5.0)) < 0.0, "reversing");
    }

    #[test]
    fn g_long_positive_when_accelerating() {
        // Accelerating forward = acceleration towards −z in the LMU frame.
        assert!((g_long(v(0.0, 0.0, -STANDARD_GRAVITY)) - 1.0).abs() < EPS);
        // Braking = acceleration towards the rear (+z).
        assert!(g_long(v(0.0, 0.0, 2.0 * STANDARD_GRAVITY)) < 0.0);
    }

    #[test]
    fn g_lat_positive_towards_the_left() {
        // A left-hand corner pulls the car towards its left side (+x in LMU).
        assert!((g_lat(v(STANDARD_GRAVITY, 0.0, 0.0)) - 1.0).abs() < EPS);
        // Right-hand corner (Gate-1 expectation: mLocalAccel.x < 0).
        assert!(g_lat(v(-1.5 * STANDARD_GRAVITY, 0.0, 0.0)) < 0.0);
        assert!((g_vert(v(0.0, STANDARD_GRAVITY, 0.0)) - 1.0).abs() < EPS);
        assert!((g_combined(3.0, 4.0) - 5.0).abs() < EPS);
    }

    #[test]
    fn rotation_rates_follow_iso() {
        // Right-hand corner: LMU yaws right (+y), ISO yaw rate negative.
        assert!(yaw_rate_radps(v(0.0, 0.5, 0.0)) < 0.0);
        assert!(
            pitch_rate_radps(v(0.2, 0.0, 0.0)) < 0.0,
            "LMU pitch up = ISO nose up"
        );
        assert!(roll_rate_radps(v(0.0, 0.0, 0.1)) > 0.0);
    }

    #[test]
    fn heading_from_orientation() {
        // Identity orientation: local forward (−z) is world −z, heading = atan2(0, −1) = π.
        let identity = [v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 0.0, 1.0)];
        let h = heading_rad(&identity);
        assert!(h.is_some_and(|h| (h.abs() - std::f64::consts::PI).abs() < EPS));
        // Car facing world +x: forward x = −ori[0].z = 1.
        let east = [v(0.0, 0.0, -1.0), v(0.0, 1.0, 0.0), v(1.0, 0.0, 0.0)];
        let h = heading_rad(&east);
        assert!(h.is_some_and(|h| (h - std::f64::consts::FRAC_PI_2).abs() < EPS));
        let zero = [v(0.0, 0.0, 0.0); 3];
        assert_eq!(heading_rad(&zero), None);
    }
}

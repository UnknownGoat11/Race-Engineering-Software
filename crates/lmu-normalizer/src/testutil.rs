//! Synthetic raw LMU structs for unit tests (decoded from zero-filled buffers of the probed layout sizes,
//! then edited field by field). Test-only; never presented as real telemetry.

use lmu_reader::offsets::{generic, scoring_info, telem_info, veh_scoring};
use lmu_reader::raw_structs::{
    decode_generic, decode_scoring_info, decode_telemetry, decode_vehicle_scoring,
};
use lmu_reader::{RawGeneric, RawScoringInfo, RawString, RawTelemetry, RawVehicleScoring, Vec3};

/// A `TelemInfoV01` with every byte zero.
pub fn zero_telemetry() -> RawTelemetry {
    decode_telemetry(&vec![0; telem_info::SIZE]).expect("zeroed telemetry slot decodes")
}

/// A `ScoringInfoV01` with every byte zero.
pub fn zero_scoring_info() -> RawScoringInfo {
    decode_scoring_info(&vec![0; scoring_info::SIZE]).expect("zeroed scoring info decodes")
}

/// A `VehicleScoringInfoV01` with every byte zero.
pub fn zero_vehicle() -> RawVehicleScoring {
    decode_vehicle_scoring(&vec![0; veh_scoring::SIZE]).expect("zeroed vehicle row decodes")
}

/// A `SharedMemoryGeneric` with every byte zero.
pub fn zero_generic() -> RawGeneric {
    decode_generic(&vec![0; generic::SIZE]).expect("zeroed generic block decodes")
}

/// A decoded C string.
pub fn text(s: &str) -> RawString {
    RawString::decode(s.as_bytes())
}

/// A vector.
pub fn v3(x: f64, y: f64, z: f64) -> Vec3 {
    Vec3 { x, y, z }
}

/// A plausible on-track player sample: GT3-like (no hybrid, no VE), moving forward at 50 m/s, tyres at
/// 80 °C (353.15 K), brakes at 400 °C (673.15 K), pressures 170 kPa.
pub fn driving_telemetry(id: i32, et: f64) -> RawTelemetry {
    let mut t = zero_telemetry();
    t.id = id;
    t.elapsed_time = et;
    t.delta_time = 0.01;
    t.lap_number = 3;
    t.lap_start_et = et - 30.0;
    t.vehicle_name = text("Test Car #7");
    t.vehicle_model = text("Test Model");
    t.track_name = text("Test Track");
    t.vehicle_class = 6;
    t.local_vel = v3(0.0, 0.0, -50.0);
    t.local_accel = v3(0.0, 0.0, -2.0);
    t.ori = [v3(1.0, 0.0, 0.0), v3(0.0, 1.0, 0.0), v3(0.0, 0.0, 1.0)];
    t.gear = 4;
    t.engine_rpm = 7_000.0;
    t.engine_max_rpm = 8_500.0;
    t.engine_torque = 400.0;
    t.engine_water_temp = 85.0;
    t.engine_oil_temp = 95.0;
    t.unfiltered_throttle = 1.0;
    t.filtered_throttle = 1.0;
    t.fuel = 60.0;
    t.fuel_capacity = 100.0;
    t.max_gears = 6;
    t.rear_brake_bias = 0.45;
    t.tc_max = 11;
    t.tc = 4;
    t.abs_max = 11;
    t.abs = 3;
    t.current_sector = 1;
    t.physical_steering_wheel_range = 540.0;
    t.visual_steering_wheel_range = 540.0;
    for w in &mut t.wheels {
        w.pressure = 170.0;
        w.temperature = [353.15, 353.15, 353.15];
        w.tire_inner_layer_temperature = [358.15, 358.15, 358.15];
        w.tire_carcass_temperature = 360.15;
        w.brake_temp = 673.15;
        w.brake_pressure = 0.0;
        w.rotation = -150.0;
        w.static_undeflected_radius = 33;
        w.wear = 0.98;
        w.tire_load = 4_000.0;
        w.optimal_temp = 85.0;
    }
    t
}

/// Session info for a race at a 13.6 km track, ambient 22 °C.
pub fn race_scoring_info(et: f64) -> RawScoringInfo {
    let mut s = zero_scoring_info();
    s.track_name = text("Test Track");
    s.session = 10;
    s.current_et = et;
    s.end_et = 3_600.0;
    s.max_laps = i32::MAX;
    s.lap_dist = 13_600.0;
    s.num_vehicles = 2;
    s.game_phase = 5;
    s.in_realtime = true;
    s.ambient_temp = 22.0;
    s.track_temp = 30.0;
    s
}

/// One scoring row.
pub fn row(id: i32, place: u8, class: &str, is_player: bool) -> RawVehicleScoring {
    let mut r = zero_vehicle();
    r.id = id;
    r.place = place;
    r.vehicle_class = text(class);
    r.is_player = is_player;
    r.driver_name = text(&format!("Driver {id}"));
    r.vehicle_name = text("Test Car");
    r.total_laps = 2;
    r.lap_dist = 1_360.0;
    r.sector = 1;
    r
}

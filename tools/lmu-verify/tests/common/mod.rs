//! Synthetic `.lmuraw` captures for the lmu-verify integration tests.
//!
//! A small kinematic "car" drives laps of a 1.2 km synthetic track (two straights joined by a left and a
//! right corner), starting cold in the garage and making one pit stop with refuel and new tyres. Every raw
//! field the Gate-1 checks look at is written in the documented LMU conventions (A-LMU-015/016/017/024/
//! 026/039/040 ...), so the clean run passes; each [`Faults`] flag breaks exactly one convention so a check
//! can be driven to FAIL. Frames go through `lmu-reader`'s capture writer and are replayed by
//! [`lmu_verify::analysis::analyze_reader`] exactly like a real capture.
//!
//! The data is synthetic test data only (never presented as real telemetry).

#![allow(dead_code)]

use lmu_reader::blocks::copy_frame;
use lmu_reader::capture::{FLAG_FRAME_GATE, FLAG_LOCKED};
use lmu_reader::offsets::{scoring_info, telem_info, telem_wheel, veh_scoring};
use lmu_reader::testutil::LayoutBuilder;
use lmu_reader::{CaptureWriter, CopyPlan, FrameInfo, RawBlocks};
use lmu_verify::analysis::checks::{CheckResult, Verdict};
use lmu_verify::analysis::{CaptureReport, analyze_reader};

/// Telemetry step (s): LMU's 100 Hz.
pub const DT_S: f64 = 0.01;
/// Scoring is published every this many telemetry frames (5 Hz).
pub const SCORING_EVERY: u64 = 20;
/// Track length (m).
pub const TRACK_M: f64 = 1_200.0;
const LEFT_CORNER: (f64, f64) = (300.0, 450.0);
const RIGHT_CORNER: (f64, f64) = (900.0, 1_050.0);
const CORNER_RADIUS_M: f64 = 70.0;
const CORNER_MPS: f64 = 30.0;
const TOP_MPS: f64 = 50.0;
const PIT_MPS: f64 = 20.0;
const ACCEL_MPS2: f64 = 4.0;
const BRAKE_MPS2: f64 = 14.0;
/// Pit lane: from here to the line, then from the line to [`PIT_EXIT_M`].
const PIT_ENTRY_M: f64 = 1_100.0;
const PIT_EXIT_M: f64 = 200.0;
/// Garage / pit box position (m).
const BOX_M: f64 = 60.0;
const SECTOR_M: f64 = 400.0;
const AMBIENT_C: f64 = 22.0;
const KELVIN: f64 = 273.15;
const GRAVITY: f64 = 9.80665;
const WHEEL_RADIUS_M: f64 = 0.34;
const WHEEL_RADIUS_CM: u8 = 34;
const REAR_BIAS: f64 = 0.45;
const START_ET_S: f64 = 10.0;
const PLAYER_ID: i32 = 7;

/// One broken convention per flag.
#[derive(Debug, Clone, Copy, Default)]
pub struct Faults {
    /// Brake temperatures written in °C (A-LMU-015 says K).
    pub brake_temp_celsius: bool,
    /// Tyre temperatures written in °C (A-LMU-049 says K).
    pub tyre_temp_celsius: bool,
    /// Tyre pressure in psi (A-LMU-049 says kPa).
    pub tyre_pressure_psi: bool,
    /// Brake pressure written as a line pressure in kPa (A-LMU-017 says 0..1).
    pub brake_pressure_kpa: bool,
    /// Virtual energy written in percent (A-LMU-013 says 0..1).
    pub ve_percent: bool,
    /// Battery charge fraction written in percent.
    pub battery_percent: bool,
    /// `mLocalAccel.z` sign flipped.
    pub invert_g_long: bool,
    /// `mLocalAccel.x` sign flipped (yaw rate and loads unchanged).
    pub invert_g_lat: bool,
    /// Steering sign flipped.
    pub invert_steering: bool,
    /// `mRotation` positive rolling forward.
    pub rotation_positive: bool,
    /// Scoring lap distance steps back now and then.
    pub lap_dist_backwards: bool,
    /// Scoring `mSector` equal to the zero-based telemetry sector (A-LMU-026 says (S + 1) mod 3).
    pub sector_direct: bool,
    /// Fuel rises while driving.
    pub fuel_rises_on_track: bool,
    /// `mInPits` false in the pit lane.
    pub in_pits_wrong: bool,
    /// `mPitState` 6 (a value LMU was never seen to use) in the garage.
    pub pit_state_unknown: bool,
    /// Right wheels' L/C/R mirrored (inner/outer swapped on one side, A-LMU-016).
    pub tyre_mirror_right: bool,
    /// Tyre wear jitters up and down.
    pub wear_jitter: bool,
    /// Engine rpm frozen while driving.
    pub frozen_rpm: bool,
    /// ET jumps back 3 s in the middle of the run (no session restart).
    pub et_regression: bool,
    /// Capture clock advances 30 ms per frame (telemetry looks like 33 Hz, scoring 1.7 Hz).
    pub slow_capture_clock: bool,
    /// Telemetry `mLapNumber` is garbage (`i32::MAX`) in every frame.
    pub garbage_lap_number: bool,
}

/// Scenario.
#[derive(Debug, Clone, Copy)]
pub struct Scenario {
    /// Start cold in the garage (otherwise on track at speed, no pit stop).
    pub garage_start: bool,
    /// Stationary time in the garage (s).
    pub garage_s: f64,
    /// Track start only: lap distance at the first frame (m; None: on the back straight).
    pub start_m: Option<f64>,
    /// The pit stop is made at the end of this lap (`None`: no stop).
    pub pit_lap: Option<i32>,
    /// Stop after this many completed laps (and half of the next one).
    pub laps: i32,
    /// Hard frame limit (also ends a run early, e.g. the short inconclusive run).
    pub max_frames: u64,
    /// Broken conventions.
    pub faults: Faults,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            garage_start: true,
            garage_s: 4.0,
            start_m: None,
            pit_lap: Some(1),
            laps: 3,
            max_frames: 40_000,
            faults: Faults::default(),
        }
    }
}

impl Scenario {
    /// The clean run with `faults`.
    pub fn with(faults: Faults) -> Self {
        Self {
            faults,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Garage { until_et: f64 },
    PitOut,
    Track,
    PitIn,
    PitBox { until_et: f64 },
}

/// Kinematic state of the synthetic car.
#[derive(Debug, Clone)]
struct Car {
    phase: Phase,
    et: f64,
    s: f64,
    v: f64,
    a_long: f64,
    corner_blend: f64,
    heading: f64,
    pos: (f64, f64),
    lap: i32,
    lap_start_et: f64,
    s1_time: f64,
    s12_time: f64,
    last_lap_time: f64,
    tel_sector: u32,
    fuel: f64,
    ve: f64,
    battery: f64,
    wear: f64,
    tyre_core_c: [f64; 4],
    edge_out: [f64; 4],
    edge_in: [f64; 4],
    brake_heat: [f64; 2],
    throttle: f64,
    brake: f64,
    pit_requested: bool,
}

fn in_range(s: f64, (a, b): (f64, f64)) -> bool {
    (a..b).contains(&s)
}

/// Corner direction at `s`: +1 left, −1 right, 0 straight.
fn corner_dir(s: f64) -> f64 {
    if in_range(s, LEFT_CORNER) {
        1.0
    } else if in_range(s, RIGHT_CORNER) {
        -1.0
    } else {
        0.0
    }
}

fn ahead(from: f64, to: f64) -> f64 {
    (to - from).rem_euclid(TRACK_M)
}

/// Highest speed that still lets the car brake down to `limit` at a point `d` metres ahead.
fn brake_curve(limit: f64, d: f64) -> f64 {
    (limit * limit + 2.0 * BRAKE_MPS2 * d).sqrt()
}

fn approach(value: f64, target: f64, tau_s: f64) -> f64 {
    value + (target - value) * (DT_S / tau_s).min(1.0)
}

impl Car {
    fn new(garage: bool, garage_s: f64, start_m: Option<f64>) -> Self {
        let (phase, s, v) = if garage {
            (
                Phase::Garage {
                    until_et: START_ET_S + garage_s,
                },
                BOX_M,
                0.0,
            )
        } else {
            // On the back straight, well before its braking zone.
            (Phase::Track, start_m.unwrap_or(LEFT_CORNER.1 + 150.0), 40.0)
        };
        let tyre = if garage { AMBIENT_C } else { 80.0 };
        Self {
            phase,
            et: START_ET_S,
            s,
            v,
            a_long: 0.0,
            corner_blend: 0.0,
            heading: 0.0,
            pos: (0.0, s),
            lap: 0,
            lap_start_et: START_ET_S,
            s1_time: 0.0,
            s12_time: 0.0,
            last_lap_time: 0.0,
            tel_sector: (s / SECTOR_M) as u32,
            fuel: 40.0,
            ve: 0.95,
            battery: 0.5,
            wear: 1.0,
            tyre_core_c: [tyre; 4],
            edge_out: [-5.0; 4],
            edge_in: [5.0; 4],
            brake_heat: [if garage { 0.0 } else { 300.0 }; 2],
            throttle: 0.0,
            brake: 0.0,
            pit_requested: false,
        }
    }

    fn in_pit_lane(&self) -> bool {
        match self.phase {
            Phase::Garage { .. } | Phase::PitBox { .. } | Phase::PitOut => true,
            Phase::PitIn => self.s >= PIT_ENTRY_M || self.s < PIT_EXIT_M,
            Phase::Track => false,
        }
    }

    fn allowed_speed(&self) -> f64 {
        let mut v = TOP_MPS;
        for corner in [LEFT_CORNER, RIGHT_CORNER] {
            v = v.min(if in_range(self.s, corner) {
                CORNER_MPS
            } else {
                brake_curve(CORNER_MPS, ahead(self.s, corner.0))
            });
        }
        match self.phase {
            Phase::Garage { .. } | Phase::PitBox { .. } => 0.0,
            Phase::PitOut => v.min(PIT_MPS),
            Phase::PitIn => {
                let lane = if self.in_pit_lane() {
                    PIT_MPS
                } else {
                    brake_curve(PIT_MPS, ahead(self.s, PIT_ENTRY_M))
                };
                let to_box = ahead(self.s, BOX_M);
                v.min(lane).min(brake_curve(0.0, to_box))
            }
            Phase::Track => v,
        }
    }

    /// Advances one telemetry step.
    fn step(&mut self, pit_lap: Option<i32>) {
        let allowed = self.allowed_speed();
        let a = ((allowed - self.v) / DT_S).clamp(-BRAKE_MPS2, ACCEL_MPS2);
        let mut a = if self.v + a * DT_S < 0.0 {
            -self.v / DT_S
        } else {
            a
        };
        // Arriving at the pit box: stop exactly on the mark (the braking curve only approaches it).
        let arriving = self.phase == Phase::PitIn
            && self.v < 5.0
            && ahead(self.s, BOX_M) <= self.v * DT_S + 0.05;
        if arriving {
            a = -self.v / DT_S;
        }
        self.a_long = a;
        (self.throttle, self.brake) = if a > 0.01 {
            ((a / ACCEL_MPS2).min(1.0), 0.0)
        } else if a < -0.01 {
            (0.0, (-a / BRAKE_MPS2).min(1.0))
        } else if self.v > 0.5 {
            (0.35, 0.0)
        } else {
            (0.0, 0.0)
        };
        self.v = (self.v + a * DT_S).max(0.0);
        let ds = self.v * DT_S;
        self.et += DT_S;
        let prev_s = self.s;
        self.s = if arriving {
            BOX_M
        } else {
            (self.s + ds).rem_euclid(TRACK_M)
        };
        let dir = corner_dir(self.s);
        self.corner_blend = approach(self.corner_blend, dir, 0.15);
        let yaw = self.corner_blend * self.v / CORNER_RADIUS_M;
        self.heading += yaw * DT_S;
        self.pos.0 += self.v * self.heading.sin() * DT_S;
        self.pos.1 += self.v * self.heading.cos() * DT_S;

        // Line crossing: new lap.
        if self.s < prev_s {
            self.lap += 1;
            self.last_lap_time = self.et - self.lap_start_et;
            self.lap_start_et = self.et;
            self.s1_time = 0.0;
            self.s12_time = 0.0;
        }
        let sector = ((self.s / SECTOR_M) as u32).min(2);
        if sector != self.tel_sector {
            match (self.tel_sector, sector) {
                (0, 1) => self.s1_time = self.et - self.lap_start_et,
                (1, 2) => self.s12_time = self.et - self.lap_start_et,
                _ => {}
            }
            self.tel_sector = sector;
        }

        // Phase transitions.
        self.phase = match self.phase {
            Phase::Garage { until_et } if self.et >= until_et => Phase::PitOut,
            Phase::PitOut if self.s >= PIT_EXIT_M && self.s < PIT_ENTRY_M => Phase::Track,
            Phase::Track if Some(self.lap) == pit_lap && self.s >= RIGHT_CORNER.0 => {
                self.pit_requested = true;
                Phase::PitIn
            }
            Phase::PitIn if arriving => Phase::PitBox {
                until_et: self.et + 6.0,
            },
            Phase::PitBox { until_et } if self.et >= until_et => {
                self.pit_requested = false;
                Phase::PitOut
            }
            p => p,
        };

        // Consumables and services.
        self.fuel -= 0.0025 * ds;
        self.ve -= 0.03 * ds / TRACK_M;
        self.wear -= 2e-5 * ds;
        if let Phase::PitBox { until_et } = self.phase {
            self.fuel += 2.5 * DT_S;
            self.ve = (self.ve + 0.02 * DT_S).min(0.95);
            // New tyres two seconds into the stop.
            if self.et >= until_et - 4.0 {
                self.wear = 1.0;
            }
        }
        self.battery = (self.battery + (0.08 * self.brake - 0.05 * self.throttle.powi(4)) * DT_S)
            .clamp(0.1, 0.95);

        // Temperatures.
        let moving = self.v > 1.0;
        for i in 0..4 {
            let target = if moving { 85.0 } else { AMBIENT_C };
            self.tyre_core_c[i] = approach(
                self.tyre_core_c[i],
                target,
                if moving { 20.0 } else { 200.0 },
            );
            // Camber: the inner edge runs 10 °C hotter on straights; in a corner the loaded (outside)
            // tyre's outer edge and the unloaded (inside) tyre's inner edge warm by 6 °C.
            let left_wheel = i.is_multiple_of(2);
            let loaded = (dir > 0.0 && !left_wheel) || (dir < 0.0 && left_wheel);
            let cornering = dir != 0.0 && self.v > 20.0;
            let (out_target, in_target) = match (cornering, loaded) {
                (true, true) => (1.0, 5.0),
                (true, false) => (-5.0, 11.0),
                (false, _) => (-5.0, 5.0),
            };
            self.edge_out[i] = approach(self.edge_out[i], out_target, 0.5);
            self.edge_in[i] = approach(self.edge_in[i], in_target, 0.5);
        }
        for (axle, gain) in [(0usize, 0.8), (1, 0.6)] {
            let h = &mut self.brake_heat[axle];
            *h += self.brake * self.v * gain * DT_S;
            *h -= *h * (0.01 + 0.0015 * self.v) * DT_S;
        }
    }
}

/// Writes the car into the image (telemetry every frame, scoring when `scoring`).
fn write_frame(b: &mut LayoutBuilder, car: &Car, f: &Faults, frame: u64, scoring: bool) {
    let t = LayoutBuilder::telem_base(0);
    let g_lat = car.corner_blend * car.v * car.v / CORNER_RADIUS_M;
    let yaw = car.corner_blend * car.v / CORNER_RADIUS_M;
    let sign = |flip: bool| if flip { -1.0 } else { 1.0 };
    b.set(t, telem_info::ELAPSED_TIME, car.et);
    let lap_number = if f.garbage_lap_number {
        i32::MAX
    } else {
        car.lap
    };
    b.set(t, telem_info::LAP_NUMBER, lap_number);
    b.set(t, telem_info::LAP_START_ET, car.lap_start_et);
    b.set_elem(t, telem_info::POS, 0, car.pos.0);
    b.set_elem(t, telem_info::POS, 2, car.pos.1);
    // LMU local frame: +x left, +z rear; yaw rate + = left is −mLocalRot.y.
    b.set_elem(t, telem_info::LOCAL_VEL, 2, -car.v);
    b.set_elem(t, telem_info::LOCAL_ACCEL, 0, sign(f.invert_g_lat) * g_lat);
    b.set_elem(
        t,
        telem_info::LOCAL_ACCEL,
        2,
        sign(f.invert_g_long) * -car.a_long,
    );
    b.set_elem(t, telem_info::LOCAL_ROT, 1, -yaw);
    let steering = sign(f.invert_steering) * -0.12 * car.corner_blend;
    b.set(t, telem_info::UNFILTERED_STEERING, steering);
    b.set(t, telem_info::FILTERED_STEERING, steering);
    b.set(t, telem_info::UNFILTERED_THROTTLE, car.throttle);
    b.set(t, telem_info::FILTERED_THROTTLE, car.throttle);
    b.set(t, telem_info::UNFILTERED_BRAKE, car.brake);
    b.set(t, telem_info::FILTERED_BRAKE, car.brake);
    let rpm = if f.frozen_rpm {
        7_000.0
    } else {
        2_500.0 + 120.0 * car.v
    };
    b.set(t, telem_info::ENGINE_RPM, rpm);
    let fuel_bump =
        if f.fuel_rises_on_track && car.phase == Phase::Track && frame.is_multiple_of(50) {
            0.6
        } else {
            0.0
        };
    b.set(
        t,
        telem_info::FUEL,
        car.fuel + fuel_bump * (frame / 50) as f64,
    );
    b.set(t, telem_info::REAR_BRAKE_BIAS, REAR_BIAS);
    let ve = if f.ve_percent { car.ve * 100.0 } else { car.ve };
    b.set(t, telem_info::VIRTUAL_ENERGY, ve as f32);
    b.set(t, telem_info::SOC, (100.0 * car.battery) as f32);
    let battery = if f.battery_percent {
        100.0 * car.battery
    } else {
        car.battery
    };
    b.set(t, telem_info::BATTERY_CHARGE_FRACTION, battery);
    let motor_state: u8 = if car.brake > 0.1 {
        3
    } else if car.throttle > 0.5 {
        2
    } else {
        1
    };
    b.set(t, telem_info::ELECTRIC_BOOST_MOTOR_STATE, motor_state);
    let sector_bits = car.tel_sector | if car.in_pit_lane() { 1 << 31 } else { 0 };
    b.set(t, telem_info::CURRENT_SECTOR, sector_bits);

    let g = g_lat / GRAVITY;
    let g_long = car.a_long / GRAVITY;
    for i in 0..4 {
        let w = LayoutBuilder::wheel_base(0, i);
        let left_wheel = i.is_multiple_of(2);
        let front = i < 2;
        let rot = car.v / WHEEL_RADIUS_M;
        b.set(
            w,
            telem_wheel::ROTATION,
            if f.rotation_positive { rot } else { -rot },
        );
        b.set(w, telem_wheel::STATIC_UNDEFLECTED_RADIUS, WHEEL_RADIUS_CM);
        let static_load = if front { 3_500.0 } else { 4_000.0 };
        let lateral = if left_wheel {
            -1_200.0 * g
        } else {
            1_200.0 * g
        };
        let longitudinal = if front {
            -600.0 * g_long
        } else {
            600.0 * g_long
        };
        b.set(
            w,
            telem_wheel::TIRE_LOAD,
            static_load + lateral + longitudinal,
        );
        let share = if front { 1.0 - REAR_BIAS } else { REAR_BIAS };
        let pressure = car.brake * share * if f.brake_pressure_kpa { 2_000.0 } else { 1.0 };
        b.set(w, telem_wheel::BRAKE_PRESSURE, pressure);
        let offset_k = if f.brake_temp_celsius { 0.0 } else { KELVIN };
        let heat = car.brake_heat[usize::from(!front)];
        b.set(
            w,
            telem_wheel::BRAKE_TEMP,
            offset_k + AMBIENT_C + heat + 0.5 * i as f64,
        );
        let tyre_k = if f.tyre_temp_celsius { 0.0 } else { KELVIN };
        let core = car.tyre_core_c[i];
        let (outer, inner) = (core + car.edge_out[i], core + car.edge_in[i]);
        // A-LMU-016: car-frame L/C/R; the left wheels' left edge is their outer edge.
        let left_is_outer = left_wheel || f.tyre_mirror_right;
        let (l, r) = if left_is_outer {
            (outer, inner)
        } else {
            (inner, outer)
        };
        for (k, c) in [l, core, r].into_iter().enumerate() {
            b.set_elem(w, telem_wheel::TEMPERATURE, k, tyre_k + c);
            b.set_elem(
                w,
                telem_wheel::TIRE_INNER_LAYER_TEMPERATURE,
                k,
                tyre_k + c - 1.0,
            );
        }
        b.set(
            w,
            telem_wheel::TIRE_CARCASS_TEMPERATURE,
            tyre_k + core - 3.0,
        );
        let kpa = 165.0 + 0.15 * (core - AMBIENT_C);
        b.set(
            w,
            telem_wheel::PRESSURE,
            if f.tyre_pressure_psi {
                kpa * 0.145_038
            } else {
                kpa
            },
        );
        let jitter = if f.wear_jitter && frame.is_multiple_of(2) {
            3e-4
        } else {
            0.0
        };
        b.set(w, telem_wheel::WEAR, car.wear + jitter);
    }

    if scoring {
        let s = LayoutBuilder::scoring_base();
        b.scoring_et(car.et);
        b.set(s, scoring_info::LAP_DIST, TRACK_M);
        b.set(s, scoring_info::AMBIENT_TEMP, AMBIENT_C);
        let r = LayoutBuilder::vehicle_base(0);
        let back = if f.lap_dist_backwards
            && car.tel_sector == 1
            && frame.is_multiple_of(2 * SCORING_EVERY)
        {
            30.0
        } else {
            0.0
        };
        b.set(r, veh_scoring::LAP_DIST, car.s - back);
        b.set(
            r,
            veh_scoring::TOTAL_LAPS,
            i16::try_from(car.lap).unwrap_or(i16::MAX),
        );
        let sector = if f.sector_direct {
            car.tel_sector
        } else {
            (car.tel_sector + 1) % 3
        };
        b.set(r, veh_scoring::SECTOR, u8::try_from(sector).unwrap_or(0));
        b.set(r, veh_scoring::CUR_SECTOR1, car.s1_time);
        b.set(r, veh_scoring::CUR_SECTOR2, car.s12_time);
        b.set(r, veh_scoring::LAST_LAP_TIME, car.last_lap_time);
        b.set(r, veh_scoring::LAP_START_ET, car.lap_start_et);
        let lane = car.in_pit_lane();
        b.set(r, veh_scoring::IN_PITS, lane && !f.in_pits_wrong);
        let garage = matches!(car.phase, Phase::Garage { .. });
        b.set(r, veh_scoring::IN_GARAGE_STALL, garage);
        let pit_state: u8 = match car.phase {
            // Observed on LMU 1.4.2.0 (A-LMU-029): 4 stopped in the box, 5 leaving it or the garage stall.
            Phase::Garage { .. } if f.pit_state_unknown => 6,
            Phase::PitBox { .. } => 4,
            Phase::Garage { .. } | Phase::PitOut => 5,
            Phase::PitIn if lane => 2,
            _ if car.pit_requested => 1,
            _ => 0,
        };
        b.set(r, veh_scoring::PIT_STATE, pit_state);
    }
}

/// Builds the capture of `scenario`.
pub fn capture(scenario: &Scenario) -> Vec<u8> {
    let mut b = LayoutBuilder::on_track_frame();
    // Only the player: keeps the synthetic frames small.
    b.num_vehicles(1);
    b.telemetry_header(1, 0, 1);
    b.set_str(
        LayoutBuilder::scoring_base(),
        scoring_info::PLAYER_NAME,
        "Synthetic Driver",
    );
    let mut car = Car::new(scenario.garage_start, scenario.garage_s, scenario.start_m);
    let mut writer = CaptureWriter::new(Vec::new(), "tool: lmu-verify synthetic test\n")
        .expect("capture header");
    let mut blocks = RawBlocks::new();
    let clock_ns = if scenario.faults.slow_capture_clock {
        30_000_000
    } else {
        10_000_000
    };
    // 30 s in: on track, after the out-lap's first corners.
    let regression_frame = 3_000;
    for frame in 0..scenario.max_frames {
        if frame > 0 {
            car.step(scenario.pit_lap);
        }
        if scenario.faults.et_regression && frame == regression_frame {
            car.et -= 3.0;
            car.lap_start_et -= 3.0;
        }
        write_frame(
            &mut b,
            &car,
            &scenario.faults,
            frame,
            frame.is_multiple_of(SCORING_EVERY),
        );
        copy_frame(b.bytes(), CopyPlan::default(), &mut blocks).expect("copy synthetic frame");
        let info = FrameInfo {
            seq: frame + 1,
            capture_mono_ns: frame * clock_ns,
            connection_id: 1,
            flags: FLAG_LOCKED | FLAG_FRAME_GATE,
        };
        writer
            .write_frame(&info, &blocks.present_blocks())
            .expect("write synthetic frame");
        if car.lap > scenario.laps && car.s > TRACK_M / 2.0 {
            break;
        }
    }
    writer.finish().expect("finish capture")
}

/// Replays `bytes` through the analysis.
pub fn analyze(bytes: &[u8]) -> CaptureReport {
    analyze_reader("synthetic.lmuraw", Some(bytes.len() as u64), bytes)
}

/// Builds and analyses `scenario`.
pub fn run(scenario: &Scenario) -> CaptureReport {
    analyze(&capture(scenario))
}

/// The result of check `id`.
pub fn check<'a>(report: &'a CaptureReport, id: &str) -> &'a CheckResult {
    report
        .checks
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no check {id}"))
}

/// Asserts the verdict of check `id`, printing its evidence on failure.
pub fn assert_verdict(report: &CaptureReport, id: &str, expected: Verdict) {
    let c = check(report, id);
    assert_eq!(
        c.verdict,
        expected,
        "{id}: {}\n{}",
        c.summary,
        c.evidence.join("\n")
    );
}

/// Every check id, in report order.
pub const CHECK_IDS: [&str; 19] = [
    "update_rates",
    "elapsed_time",
    "brake_temp_unit",
    "tyre_temperature_unit",
    "tyre_pressure_unit",
    "brake_pressure_fraction",
    "virtual_energy",
    "state_of_charge",
    "g_long_sign",
    "g_lat_sign",
    "steering_sign",
    "wheel_speed",
    "lap_distance",
    "sectors_and_laps",
    "fuel",
    "pit_flags",
    "pit_state_values",
    "tyre_side_mapping",
    "tyre_wear_direction",
];

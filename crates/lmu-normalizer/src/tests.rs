//! Normalizer-level tests with synthetic raw structs (never real telemetry).

use std::collections::BTreeSet;
use std::sync::Arc;

use lmu_reader::{
    Consistency, FrameSource, PlayerIdentity, PlayerTelemetry, RawBlocks, RawGeneric,
    RawScoringInfo, RawSnapshot, RawTelemetry, RawVehicleScoring, ScoringBlock, SupportLevel,
    TelemetryHeader, TelemetrySlots,
};
use telemetry_model::channel::{Availability, Source};
use telemetry_model::enums::SessionType;
use telemetry_model::events::SessionEvent;
use telemetry_model::registry::{self, Wheel};
use telemetry_model::{ChannelId, Quality, Value, ch};

use crate::capability::{CAPABILITY_DECISION_SAMPLES, VehicleCapabilityTracker};
use crate::detect::{BRAKE_UNIT_VOTES_REQUIRED, BrakeTempUnit, FractionScale, UnitDecisionState};
use crate::mapping::{
    self, DERIVED_HERE, DERIVED_HERE_PER_WHEEL, MapInput, MapState, SessionDetectors, wheel_id,
};
use crate::normalizer::{
    NormalizedOutput, Normalizer, NormalizerInput, SUSTAINED_REJECTION_FRAMES, TelemetryDisposition,
};
use crate::notice::{NoticeCode, NoticeLog};
use crate::testutil::{driving_telemetry, race_scoring_info, row, text, zero_generic};
use crate::writer::FrameWriter;

const PLAYER_ID: i32 = 7;
const LMU_1_4_2_VERSION: i32 = 14_200;
/// A future LMU version nobody has verified: the evidence-based detectors run without any verified default.
const UNVERIFIED_FUTURE_VERSION: i32 = 14_300;

/// Owns the raw blocks of one step.
struct Fixture {
    generic: RawGeneric,
    info: RawScoringInfo,
    rows: Vec<RawVehicleScoring>,
    header: TelemetryHeader,
    telemetry: Option<RawTelemetry>,
}

impl Fixture {
    fn driving(et: f64) -> Self {
        let mut generic = zero_generic();
        generic.game_version = LMU_1_4_2_VERSION;
        Self {
            generic,
            info: race_scoring_info(et),
            rows: vec![
                row(3, 1, "Hyper", false),
                row(PLAYER_ID, 2, "GT3", true),
                row(9, 3, "GT3", false),
            ],
            header: TelemetryHeader {
                active_vehicles: 3,
                player_vehicle_idx: 1,
                player_has_vehicle: 1,
            },
            telemetry: Some(driving_telemetry(PLAYER_ID, et)),
        }
    }

    fn t(&mut self) -> &mut RawTelemetry {
        self.telemetry
            .get_or_insert_with(|| driving_telemetry(PLAYER_ID, 0.0))
    }

    /// Advances both clocks by `dt`.
    fn advance(&mut self, dt: f64) {
        self.info.current_et += dt;
        let t = self.t();
        t.elapsed_time += dt;
        t.lap_start_et += dt;
    }

    fn input(&self, scoring_updated: bool) -> NormalizerInput<'_> {
        NormalizerInput {
            capture_mono_ns: 1_000,
            generic: &self.generic,
            scoring_info: &self.info,
            vehicles: &self.rows,
            scoring_updated,
            telemetry_header: &self.header,
            player: self.telemetry.as_ref(),
            version_verified: true,
        }
    }

    fn step(&self, n: &mut Normalizer) -> NormalizedOutput {
        n.process_input(&self.input(true))
    }
}

fn f(out: &NormalizedOutput, id: ChannelId) -> Option<f64> {
    out.frame.as_ref().and_then(|fr| fr.f64(id))
}

fn q(out: &NormalizedOutput, id: ChannelId) -> Option<Quality> {
    out.frame.as_ref().map(|fr| fr.get(id).quality)
}

/// Equal within f32 storage precision (1e-3 absolute, or 1e-6 relative for large values).
fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-3_f64.max(b.abs() * 1e-6))
}

/// Maps `steps` samples of `fx` through one set of detectors (capabilities observed as the normalizer
/// does); returns the channels considered and the channels that received a real value in any step.
fn map_coverage(fx: &Fixture, steps: u32) -> (BTreeSet<ChannelId>, BTreeSet<ChannelId>) {
    let telemetry = fx.telemetry.clone().expect("player telemetry");
    let mut det = SessionDetectors::default();
    let mut log = NoticeLog::default();
    let mut caps = VehicleCapabilityTracker::default();
    let (mut touched, mut valued) = (BTreeSet::new(), BTreeSet::new());
    for i in 0..steps {
        let mut t = telemetry.clone();
        t.elapsed_time += f64::from(i) * 0.01;
        caps.observe(&t);
        let mut frame = telemetry_model::TelemetryFrame::new(1, 0, Some(t.elapsed_time));
        let mut writer = FrameWriter::new(&mut frame);
        let mut notices = Vec::new();
        let input = MapInput {
            telemetry: &t,
            header: &fx.header,
            generic: &fx.generic,
            info: &fx.info,
            player_row: fx.rows.iter().find(|r| r.is_player),
            class_place: Some(1),
            caps: caps.capabilities(),
        };
        let mut st = MapState {
            det: &mut det,
            notice_log: &mut log,
            notices: &mut notices,
        };
        mapping::map_frame(&input, &mut st, &mut writer);
        touched.extend(writer.touched.iter().copied());
        valued.extend(writer.valued.iter().copied());
        assert!(writer.into_errors().is_empty(), "no mapping bug");
    }
    (touched, valued)
}

/// A hybrid Hypercar-like fixture with every optional system active and no sentinel left in place.
fn hybrid_everything_fixture() -> Fixture {
    let mut fx = Fixture::driving(100.0);
    fx.info.max_laps = 50;
    fx.info.time_of_day = 52_200.0;
    // A third classified car, so the player has a place behind (place gaps are no data for the last car).
    fx.info.num_vehicles = 3;
    fx.generic.app_info.options_page = text("Telemetry");
    let row = &mut fx.rows[1];
    row.best_lap_time = 210.0;
    row.last_lap_time = 211.0;
    row.best_sector1 = 70.0;
    row.best_sector2 = 140.0;
    row.last_sector1 = 71.0;
    row.last_sector2 = 141.0;
    row.cur_sector1 = 72.0;
    row.cur_sector2 = 142.0;
    row.best_lap_sector1 = 70.0;
    row.best_lap_sector2 = 140.0;
    row.time_into_lap = 30.0;
    row.estimated_lap_time = 210.0;
    row.lap_start_et = 70.0;
    row.time_behind_next = 1.5;
    row.time_behind_leader = 3.0;
    row.qualification = 4;
    row.veh_filename = text("car.veh");
    row.pit_lap_dist = 100.0;
    let t = fx.t();
    t.vehicle_class = 0;
    t.electric_boost_motor_state = 2;
    t.electric_boost_motor_torque = 150.0;
    t.electric_boost_motor_rpm = 12_000.0;
    t.electric_boost_motor_temperature = 70.0;
    t.electric_boost_water_temperature = 60.0;
    t.regen = 120.0;
    t.soc = 55.0; // percent
    t.battery_charge_fraction = 0.55;
    t.virtual_energy = 0.87; // 0..1, verified for LMU 1.4.2.0 (A-LMU-013)
    t.rear_flap_activated = 1;
    t.turbo_boost_pressure = 180.0;
    // Chassis and aero values a car that fills them reports (0 m would be impossible at rest, A-LMU-065).
    (t.front_ride_height, t.rear_ride_height, t.front_wing_height) = (0.045, 0.06, 0.09);
    (t.drag, t.front_downforce, t.rear_downforce) = (4_000.0, 6_000.0, 7_000.0);
    (t.tc_slip_max, t.tc_slip, t.tc_cut_max, t.tc_cut) = (5, 2, 5, 1);
    (t.migration_max, t.migration, t.motor_map_max, t.motor_map) = (3, 1, 4, 2);
    (t.front_anti_sway_max, t.front_anti_sway) = (5, 3);
    (t.rear_anti_sway_max, t.rear_anti_sway) = (5, 2);
    t.front_tire_compound_name = text("Medium");
    t.rear_tire_compound_name = text("Medium");
    for w in &mut t.wheels {
        w.brake_temp = 24.0; // impossible as Kelvin: the unit is decided (Celsius) within the run
        w.terrain_name = text("ROAD");
        // Tyre forces and sliding a car that fills them reports (a bit-exact 0 while moving is withheld).
        (w.lateral_force, w.longitudinal_force, w.grip_fract) = (-850.0, 1_200.0, 0.12);
    }
    fx
}

#[test]
fn every_lmu_channel_and_normalizer_formula_is_mapped() {
    let gt3 = Fixture::driving(100.0);
    let hybrid = hybrid_everything_fixture();
    let (mut touched, mut valued) = map_coverage(&gt3, 1);
    let (touched_h, valued_h) = map_coverage(&hybrid, BRAKE_UNIT_VOTES_REQUIRED);
    touched.extend(touched_h);
    valued.extend(valued_h);

    let mut expected: Vec<ChannelId> = registry::all()
        .iter()
        .filter(|d| d.source.is_lmu_shared_memory() && d.availability != Availability::NotProvided)
        .map(|d| d.id)
        .collect();
    expected.extend_from_slice(DERIVED_HERE);
    for &fl in DERIVED_HERE_PER_WHEEL {
        expected.extend(Wheel::ALL.map(|w| wheel_id(fl, w)));
    }
    let name = |id: &ChannelId| registry::by_id(*id).map(|d| d.name);
    let missing: Vec<&str> = expected
        .iter()
        .filter(|id| !touched.contains(id))
        .filter_map(name)
        .collect();
    assert!(missing.is_empty(), "unmapped channels: {missing:?}");
    // Considered is not enough: every channel must carry a real value in at least one fixture (a mapping
    // turned into "no data" or a value-less quality by mistake fails here).
    let never_valued: Vec<&str> = expected
        .iter()
        .filter(|id| !valued.contains(id))
        .filter_map(name)
        .collect();
    assert!(
        never_valued.is_empty(),
        "channels never written with a value: {never_valued:?}"
    );

    // Nothing outside LMU channels, normalizer formulas and the source counters is written.
    let foreign: Vec<&str> = touched
        .iter()
        .filter_map(|&id| registry::by_id(id))
        .filter(|d| d.source == Source::Calculated && !expected.contains(&d.id))
        .map(|d| d.name)
        .collect();
    assert!(
        foreign.is_empty(),
        "race-engines/experimental channels written: {foreign:?}"
    );
}

#[test]
fn zero_steering_ranges_are_unavailable_not_invalid() {
    let mut fx = Fixture::driving(100.0);
    let t = fx.telemetry.as_mut().expect("player telemetry");
    t.physical_steering_wheel_range = 0.0;
    t.visual_steering_wheel_range = 0.0;
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert!(
        out.rejections.is_empty(),
        "an unset range is not a rejection: {:?}",
        out.rejections
    );
    for id in [
        ch::STEERING_RANGE_PHYSICAL,
        ch::STEERING_RANGE_VISUAL,
        ch::STEERING_ANGLE,
    ] {
        assert_eq!(q(&out, id), Some(Quality::Unavailable), "{id:?}");
    }
}

#[test]
fn plausible_driving_sample_is_fully_valid_and_converted() {
    let fx = Fixture::driving(100.0);
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert_eq!(out.disposition, TelemetryDisposition::NewSample);
    assert!(
        out.rejections.is_empty(),
        "rejections: {:?}",
        out.rejections
    );
    let frame = out.frame.as_ref().expect("frame");
    let invalid: Vec<&str> = frame
        .iter()
        .filter(|(_, s)| s.quality == Quality::Invalid)
        .map(|(d, _)| d.name)
        .collect();
    assert!(invalid.is_empty(), "invalid: {invalid:?}");
    assert_eq!(frame.source_et_s, Some(100.0));

    // Units (all through telemetry_model::units::convert).
    assert!(close(f(&out, ch::TYRE_PRESSURE_FL), 170_000.0), "kPa → Pa");
    assert!(close(f(&out, ch::TYRE_TEMP_MIDDLE_RR), 80.0), "K → °C");
    assert!(close(f(&out, ch::TYRE_CARCASS_TEMP_FL), 87.0));
    assert!(
        close(f(&out, ch::BRAKE_TEMP_FL), 400.0),
        "673.15 K → 400 °C: Kelvin is verified for LMU 1.4.2.0 (Gate 1, A-LMU-015)"
    );
    assert!(close(f(&out, ch::TYRE_RADIUS_FL), 0.33), "cm → m");
    assert!(close(f(&out, ch::WHEEL_SPEED_RL), 49.5), "|rot| × radius");
    assert!(close(f(&out, ch::SPEED), 50.0), "|mLocalVel|");
    assert!(close(f(&out, ch::SPEED_FORWARD), 50.0));
    assert!(
        close(f(&out, ch::G_LONG), 2.0 / 9.806_65),
        "accelerating = +"
    );
    assert!(close(
        f(&out, ch::STEERING_RANGE_PHYSICAL),
        540.0_f64.to_radians()
    ));
    assert!(close(f(&out, ch::FUEL_FRACTION), 0.6));
    assert!(close(f(&out, ch::LAP_TIME), 30.0));
    assert!(close(f(&out, ch::SESSION_TIME_REMAINING_CALC), 3_500.0));
    assert!(close(
        f(&out, ch::ENGINE_POWER),
        400.0 * 7_000.0 * std::f64::consts::TAU / 60.0
    ));
    // Scoring-derived player channels and the CALCULATED class place.
    assert_eq!(frame.get(ch::POSITION).value, Value::U8(2));
    assert_eq!(frame.get(ch::CLASS_POSITION).value, Value::U8(1));
    assert!(close(f(&out, ch::DISTANCE), 2.0 * 13_600.0 + 1_360.0));
    assert!(close(f(&out, ch::LAP_PROGRESS), 0.1));
    // Sentinels stay "no data", never 0.
    assert_eq!(
        q(&out, ch::SESSION_MAX_LAPS),
        Some(Quality::Unavailable),
        "timed race"
    );
    assert_eq!(
        q(&out, ch::BEST_LAP_TIME),
        Some(Quality::Unavailable),
        "no lap yet"
    );
    // Standings are published with the first step.
    let scoring = out.scoring.as_ref().expect("standings");
    assert_eq!(scoring.player().map(|p| p.class_place), Some(Some(1)));
}

#[test]
fn tyre_edges_follow_the_wheel_side() {
    let mut fx = Fixture::driving(100.0);
    for w in &mut fx.t().wheels {
        // Car-frame left, centre, right edges.
        w.temperature = [373.15, 363.15, 353.15];
    }
    let out = fx.step(&mut Normalizer::new());
    // Left wheels: index 0 (car-left) is outboard.
    assert!(close(f(&out, ch::TYRE_TEMP_OUTER_FL), 100.0));
    assert!(close(f(&out, ch::TYRE_TEMP_INNER_FL), 80.0));
    assert!(close(f(&out, ch::TYRE_TEMP_SPREAD_RL), -20.0));
    // Right wheels: index 0 (car-left) is inboard.
    assert!(close(f(&out, ch::TYRE_TEMP_INNER_FR), 100.0));
    assert!(close(f(&out, ch::TYRE_TEMP_OUTER_RR), 80.0));
    // Raw L/C/R channels keep the car-frame meaning.
    assert!(close(f(&out, ch::TYRE_TEMP_LEFT_FR), 100.0));
    assert!(close(f(&out, ch::TYRE_TEMP_AVG_FL), 90.0));
}

#[test]
fn duplicate_et_produces_no_physics_sample() {
    let mut fx = Fixture::driving(100.0);
    let mut n = Normalizer::new();
    assert_eq!(fx.step(&mut n).disposition, TelemetryDisposition::NewSample);
    let dup = fx.step(&mut n);
    assert_eq!(dup.disposition, TelemetryDisposition::Duplicate);
    assert!(dup.frame.is_none());
    fx.advance(0.01);
    let next = fx.step(&mut n);
    assert_eq!(next.disposition, TelemetryDisposition::NewSample);
    assert_eq!(next.frame.as_ref().map(|f| f.sequence), Some(2));
    assert_eq!(n.stats().duplicate_frames, 1);
    assert_eq!(n.stats().frames_published, 2);
    assert_eq!(
        next.frame
            .as_ref()
            .map(|f| f.get(ch::SRC_DUPLICATE_FRAMES).value.clone()),
        Some(Value::U64(1))
    );
}

#[test]
fn et_regression_is_a_session_restart() {
    let mut fx = Fixture::driving(500.0);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    fx.advance(-400.0);
    let out = fx.step(&mut n);
    assert_eq!(
        out.disposition,
        TelemetryDisposition::NewSampleAfterRegression
    );
    assert!(out.frame.is_some());
    assert!(
        out.notices
            .iter()
            .any(|x| x.code == NoticeCode::EtRegression)
    );
    assert!(
        out.events
            .iter()
            .any(|e| matches!(e.event, SessionEvent::SessionChanged { .. }))
    );
    assert!(
        out.rejections.is_empty(),
        "rate guards were reset: {:?}",
        out.rejections
    );
    assert_eq!(n.stats().et_regressions, 1);
}

#[test]
fn non_finite_values_are_invalid_never_zero() {
    let mut fx = Fixture::driving(100.0);
    fx.t().engine_rpm = f64::NAN;
    fx.t().wheels[1].pressure = f64::INFINITY;
    fx.t().local_vel.z = f64::NAN;
    let out = fx.step(&mut Normalizer::new());
    for id in [
        ch::RPM,
        ch::TYRE_PRESSURE_FR,
        ch::SPEED,
        ch::LOCAL_VEL_Z,
        ch::ENGINE_POWER,
    ] {
        let s = out
            .frame
            .as_ref()
            .map(|fr| fr.get(id).clone())
            .expect("frame");
        assert_eq!(
            (s.quality, s.value),
            (Quality::Invalid, Value::None),
            "{id:?}"
        );
    }
    assert_eq!(
        q(&out, ch::TYRE_PRESSURE_FL),
        Some(Quality::Valid),
        "other wheels unaffected"
    );

    let mut bad_et = Fixture::driving(100.0);
    bad_et.t().elapsed_time = f64::NAN;
    let out = bad_et.step(&mut Normalizer::new());
    assert_eq!(out.disposition, TelemetryDisposition::NonFiniteEt);
    assert!(out.frame.is_none());
}

#[test]
fn out_of_range_and_jumps_are_rejected_by_registry_validation() {
    let mut fx = Fixture::driving(100.0);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    fx.advance(0.01);
    fx.t().engine_water_temp = 900.0; // above the 200 °C plausibility bound
    fx.t().fuel = 10.0; // 50 L lost in 10 ms
    let out = fx.step(&mut n);
    assert_eq!(q(&out, ch::WATER_TEMP), Some(Quality::Invalid));
    assert_eq!(q(&out, ch::FUEL), Some(Quality::Invalid));
    assert!(out.rejections.iter().any(|(id, _)| *id == ch::WATER_TEMP));
}

#[test]
fn non_hybrid_car_reports_hybrid_channels_not_supported() {
    let mut fx = Fixture::driving(100.0);
    let mut n = Normalizer::new();
    let first = fx.step(&mut n);
    // Undecided: unavailable, never 0.
    assert_eq!(
        q(&first, ch::HYBRID_MOTOR_TORQUE),
        Some(Quality::Unavailable)
    );
    assert_eq!(q(&first, ch::VIRTUAL_ENERGY), Some(Quality::Unavailable));
    let mut last = first;
    for _ in 1..CAPABILITY_DECISION_SAMPLES {
        fx.advance(0.01);
        last = fx.step(&mut n);
    }
    let caps = n.vehicle_capabilities();
    assert_eq!(caps.supports_hybrid, Some(false));
    assert_eq!(caps.supports_virtual_energy, Some(false));
    for id in crate::capability::HYBRID_GATED {
        assert_eq!(q(&last, id), Some(Quality::NotSupported), "{id:?}");
    }
    assert_eq!(q(&last, ch::VIRTUAL_ENERGY), Some(Quality::NotSupported));
    assert_eq!(
        q(&last, ch::HYBRID_MOTOR_STATE),
        Some(Quality::Valid),
        "the evidence itself"
    );
    let cap = n.capability_map().get(ch::REGEN_POWER).expect("cap");
    assert_eq!(
        cap.status,
        telemetry_model::scoring::CapabilityStatus::NotSupported
    );
    assert!(cap.note.as_deref().is_some_and(|s| s.contains("no hybrid")));
}

#[test]
fn hybrid_car_publishes_hybrid_and_energy_channels() {
    let mut fx = Fixture::driving(100.0);
    // Percent detection from evidence: a version without a verified VE scale.
    fx.generic.game_version = UNVERIFIED_FUTURE_VERSION;
    {
        let t = fx.t();
        t.vehicle_class = 0;
        t.electric_boost_motor_state = 2;
        t.electric_boost_motor_torque = 150.0;
        t.electric_boost_motor_rpm = 12_000.0;
        t.electric_boost_motor_temperature = 70.0;
        t.electric_boost_water_temperature = 0.0; // "no cooler" sentinel
        t.regen = 120.0; // kW
        t.soc = 0.6;
        t.battery_charge_fraction = 0.6;
        t.virtual_energy = 87.0; // percent scale
    }
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert_eq!(n.vehicle_capabilities().supports_hybrid, Some(true));
    assert!(close(f(&out, ch::REGEN_POWER), 120_000.0), "kW → W");
    assert!(close(f(&out, ch::HYBRID_MOTOR_TEMP), 70.0));
    assert_eq!(
        q(&out, ch::HYBRID_WATER_TEMP),
        Some(Quality::Unavailable),
        "sentinel 0"
    );
    assert_eq!(
        q(&out, ch::BATTERY_SOC),
        Some(Quality::Unavailable),
        "0.6 fits 0..1 and percent: withheld until decided"
    );
    assert!(close(f(&out, ch::VIRTUAL_ENERGY), 0.87), "percent detected");
    assert!(out.notices.iter().any(
        |x| x.code == NoticeCode::FractionScalePercent && x.channel == Some(ch::VIRTUAL_ENERGY)
    ));
}

#[test]
fn unrecognized_energy_scale_is_invalid_with_the_raw_value_in_a_notice() {
    let mut fx = Fixture::driving(100.0);
    fx.t().virtual_energy = 4.0e6; // e.g. joules: never converted to an invented unit
    let out = fx.step(&mut Normalizer::new());
    assert_eq!(q(&out, ch::VIRTUAL_ENERGY), Some(Quality::Invalid));
    let notice = out
        .notices
        .iter()
        .find(|x| x.code == NoticeCode::FractionScaleUnrecognized)
        .expect("notice");
    assert_eq!(notice.raw_value, Some(4.0e6));
}

#[test]
fn brake_pressure_above_one_and_a_half_stops_mapping() {
    let mut fx = Fixture::driving(100.0);
    let mut n = Normalizer::new();
    fx.t().wheels[0].brake_pressure = 0.45;
    let ok = fx.step(&mut n);
    assert!(
        close(f(&ok, ch::BRAKE_PRESSURE_FL), 0.45),
        "fraction, not Pa"
    );
    fx.advance(0.01);
    fx.t().wheels[0].brake_pressure = 2_400.0;
    let changed = fx.step(&mut n);
    assert!(
        changed
            .notices
            .iter()
            .any(|x| x.code == NoticeCode::BrakePressureUnitChanged)
    );
    fx.advance(0.01);
    fx.t().wheels[0].brake_pressure = 0.3;
    let after = fx.step(&mut n);
    for wheel in Wheel::ALL {
        assert_eq!(
            q(&after, wheel_id(ch::BRAKE_PRESSURE_FL, wheel)),
            Some(Quality::Invalid),
            "latched for the session"
        );
    }
}

#[test]
fn brake_temperature_unit_is_decided_from_cold_brake_evidence() {
    let mut fx = Fixture::driving(10.0);
    fx.generic.game_version = UNVERIFIED_FUTURE_VERSION;
    fx.t().local_vel.z = 0.0; // parked
    for w in &mut fx.t().wheels {
        w.brake_temp = 24.0; // Celsius near the 22 °C ambient
    }
    let mut n = Normalizer::new();
    let first = fx.step(&mut n);
    // Undecided: withheld, never shown as a temperature in an unverified unit.
    assert_eq!(q(&first, ch::BRAKE_TEMP_FL), Some(Quality::Unavailable));
    assert!(
        first
            .notices
            .iter()
            .any(|x| x.code == NoticeCode::BrakeTempUnitUnverified)
    );
    let note = n
        .capability_map()
        .get(ch::BRAKE_TEMP_FL)
        .and_then(|c| c.note.clone());
    assert!(note.is_some_and(|s| s.contains("unit unverified")));
    let mut last = first;
    for _ in 1..BRAKE_UNIT_VOTES_REQUIRED {
        fx.advance(0.01);
        last = fx.step(&mut n);
    }
    let decision = n.brake_temp_unit();
    assert_eq!(
        (decision.state, decision.unit),
        (UnitDecisionState::Decided, BrakeTempUnit::Celsius)
    );
    assert!(
        last.notices
            .iter()
            .any(|x| x.code == NoticeCode::BrakeTempUnitDecided)
    );
    assert!(close(f(&last, ch::BRAKE_TEMP_RR), 24.0));
}

#[test]
fn sector_and_pit_lane_decoding() {
    let mut fx = Fixture::driving(100.0);
    fx.t().current_sector = i32::from_le_bytes(0x8000_0001u32.to_le_bytes());
    fx.rows[1].sector = 0; // scoring: 0 = sector 3
    let out = fx.step(&mut Normalizer::new());
    let frame = out.frame.as_ref().expect("frame");
    assert_eq!(frame.get(ch::SECTOR).value, Value::U8(1));
    assert_eq!(frame.get(ch::IN_PIT_LANE).value, Value::Bool(true));
    assert_eq!(frame.get(ch::SECTOR_SCORING).value, Value::U8(2));
    assert_eq!(frame.get(ch::SECTOR_SCORING_RAW).value, Value::I8(0));
}

#[test]
fn missing_player_row_makes_player_scoring_unavailable() {
    let mut fx = Fixture::driving(100.0);
    fx.rows[1].id = 99; // mIsPlayer row with another mID
    let out = fx.step(&mut Normalizer::new());
    assert_eq!(q(&out, ch::POSITION), Some(Quality::Unavailable));
    assert_eq!(q(&out, ch::LAP_DISTANCE), Some(Quality::Unavailable));
    assert!(
        out.notices
            .iter()
            .any(|x| x.code == NoticeCode::PlayerScoringRowMissing)
    );
    assert_eq!(
        q(&out, ch::RPM),
        Some(Quality::Valid),
        "telemetry unaffected"
    );
}

#[test]
fn inconsistent_standings_are_dropped() {
    let mut fx = Fixture::driving(100.0);
    fx.rows[2].id = 3; // duplicate mID
    let out = fx.step(&mut Normalizer::new());
    assert!(out.scoring.is_none());
    assert!(
        out.notices
            .iter()
            .any(|x| x.code == NoticeCode::StandingsInconsistent)
    );
    assert_eq!(q(&out, ch::CLASS_POSITION), Some(Quality::Unavailable));
}

#[test]
fn vehicle_track_and_session_changes_emit_events() {
    let mut fx = Fixture::driving(100.0);
    let mut n = Normalizer::new();
    let first = fx.step(&mut n);
    assert_eq!(
        first.events.len(),
        3,
        "session, track and vehicle become known"
    );
    assert!(first.identity.is_some());
    fx.advance(0.01);
    assert!(fx.step(&mut n).events.is_empty());

    fx.advance(0.01);
    fx.t().vehicle_name = text("Other Car");
    let veh = fx.step(&mut n);
    assert!(matches!(
        veh.events.as_slice(),
        [e] if matches!(&e.event, SessionEvent::VehicleChanged { vehicle_name: Some(v), .. } if v == "Other Car")
    ));
    assert_eq!(n.identity().vehicle_name.as_deref(), Some("Other Car"));

    fx.advance(0.01);
    fx.info.track_name = text("Other Track");
    fx.info.session = 5;
    let both = fx.step(&mut n);
    let kinds: Vec<&SessionEvent> = both.events.iter().map(|e| &e.event).collect();
    assert!(matches!(
        kinds.as_slice(),
        [
            SessionEvent::SessionChanged { .. },
            SessionEvent::TrackChanged { .. }
        ]
    ));
    assert!(n.stats().session_resets >= 3);
}

#[test]
fn no_player_produces_no_frame_but_keeps_standings() {
    let mut fx = Fixture::driving(100.0);
    fx.telemetry = None;
    let out = fx.step(&mut Normalizer::new());
    assert_eq!(out.disposition, TelemetryDisposition::NoPlayer);
    assert!(out.frame.is_none());
    assert!(out.scoring.is_some());
}

#[test]
fn process_accepts_reader_snapshots() {
    let fx = Fixture::driving(100.0);
    let telemetry = fx.telemetry.clone().expect("telemetry");
    let snapshot = RawSnapshot {
        seq: 1,
        capture_mono_ns: 42,
        connection_id: 1,
        source: FrameSource {
            sync: None,
            consistency: Consistency::Locked,
        },
        telemetry_updated: true,
        scoring_updated: true,
        support: SupportLevel::Supported,
        version_verified: false,
        generic: fx.generic.clone(),
        scoring: Arc::new(ScoringBlock {
            info: fx.info.clone(),
            num_vehicles_raw: 3,
            scoring_stream_size_raw: 0,
            vehicles: fx.rows.clone(),
        }),
        telemetry_header: fx.header,
        player: Some(PlayerTelemetry { slot: 1, telemetry }),
        player_identity: PlayerIdentity::Verified,
        vehicle_telemetry: Arc::new(TelemetrySlots::from_blocks(&RawBlocks::new())),
        paths: None,
        scoring_stream: None,
    };
    let mut n = Normalizer::new();
    let out = n.process(&snapshot);
    assert_eq!(out.disposition, TelemetryDisposition::NewSample);
    assert_eq!(out.frame.as_ref().map(|f| f.capture_mono_ns), Some(42));
    assert!(
        out.notices
            .iter()
            .any(|x| x.code == NoticeCode::GameVersionUnverified)
    );
}

#[test]
fn capability_map_tracks_qualities_and_rates() {
    let fx = Fixture::driving(100.0);
    let mut n = Normalizer::new();
    n.set_native_rates(Some(100.0), Some(5.0));
    let out = fx.step(&mut n);
    assert!(out.capabilities_changed);
    use telemetry_model::scoring::CapabilityStatus as S;
    let status = |id| n.capability_map().get(id).map(|c| c.status);
    assert_eq!(status(ch::RPM), Some(S::Available));
    assert_eq!(status(ch::SPEED), Some(S::Calculated));
    assert_eq!(
        status(ch::BRAKE_TEMP_FL),
        Some(S::Available),
        "Kelvin verified for LMU 1.4.2.0"
    );
    assert_eq!(
        status(ch::SLIP_RATIO_FL),
        Some(S::Unavailable),
        "experimental, not published"
    );
    let rate = n
        .capability_map()
        .get(ch::POSITION)
        .and_then(|c| c.native_rate_hz);
    assert_eq!(
        rate,
        Some(5.0),
        "rates survive the session reset of the first frame"
    );
    let note = n
        .capability_map()
        .get(ch::G_LAT)
        .and_then(|c| c.note.clone());
    assert!(note.is_some_and(|s| s.contains("A-LMU-024")));
}

fn session_changes(outs: &[NormalizedOutput]) -> Vec<SessionEvent> {
    outs.iter()
        .flat_map(|o| o.events.iter().map(|e| e.event.clone()))
        .filter(|e| matches!(e, SessionEvent::SessionChanged { .. }))
        .collect()
}

#[test]
fn session_change_with_et_reset_is_reported_once_and_is_not_a_regression() {
    let mut fx = Fixture::driving(3_600.0);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    // Practice ends at ET 3600; the next session loads with the player ET back near 0 in the same step.
    fx.info.session = 5;
    fx.info.current_et = 0.5;
    fx.t().elapsed_time = 0.5;
    let out = fx.step(&mut n);
    assert_eq!(out.disposition, TelemetryDisposition::NewSample);
    assert_eq!(
        session_changes(std::slice::from_ref(&out)),
        vec![SessionEvent::SessionChanged {
            from: Some(SessionType::from_lmu(10)),
            to: SessionType::from_lmu(5)
        }]
    );
    assert!(
        !out.notices
            .iter()
            .any(|x| x.code == NoticeCode::EtRegression)
    );
    assert_eq!(n.stats().et_regressions, 0);
    assert_eq!(
        out.frame
            .as_ref()
            .map(|f| f.get(ch::SRC_ET_REGRESSIONS).value.clone()),
        Some(Value::U32(0))
    );
}

#[test]
fn session_change_across_a_no_player_loading_gap_is_reported_once() {
    let mut fx = Fixture::driving(3_600.0);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    let mut outs = Vec::new();
    // Loading: no player telemetry, the new session type already in the scoring block.
    let saved = fx.telemetry.take();
    fx.info.session = 5;
    fx.info.current_et = 0.0;
    outs.push(fx.step(&mut n));
    outs.push(fx.step(&mut n));
    fx.telemetry = saved;
    fx.t().elapsed_time = 0.5;
    fx.info.current_et = 0.5;
    for _ in 0..3 {
        outs.push(fx.step(&mut n));
        fx.advance(0.01);
    }
    assert_eq!(
        session_changes(&outs).len(),
        1,
        "{:?}",
        session_changes(&outs)
    );
    assert_eq!(n.stats().et_regressions, 0);
    assert_eq!(outs[2].disposition, TelemetryDisposition::NewSample);
}

#[test]
fn telemetry_restart_before_the_scoring_refresh_is_reported_once() {
    let mut fx = Fixture::driving(3_600.0);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    // Telemetry restarts first (scoring block unchanged, still the old session)...
    fx.t().elapsed_time = 0.5;
    let restart = n.process_input(&fx.input(false));
    assert_eq!(
        restart.disposition,
        TelemetryDisposition::NewSampleAfterRegression
    );
    assert!(restart.events.is_empty(), "restart event deferred");
    // ...then the scoring block reports the new session: one SessionChanged, the real transition.
    fx.info.session = 5;
    fx.info.current_et = 0.5;
    fx.t().elapsed_time = 0.51;
    let change = fx.step(&mut n);
    assert_eq!(
        session_changes(&[restart, change]),
        vec![SessionEvent::SessionChanged {
            from: Some(SessionType::from_lmu(10)),
            to: SessionType::from_lmu(5)
        }]
    );
}

#[test]
fn restart_of_the_same_session_is_reported_after_the_scoring_refresh() {
    let mut fx = Fixture::driving(500.0);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    fx.advance(-400.0);
    let restart = n.process_input(&fx.input(false));
    assert!(restart.events.is_empty());
    fx.advance(0.01);
    let refreshed = fx.step(&mut n);
    let race = SessionType::from_lmu(10);
    assert_eq!(
        session_changes(&[restart, refreshed]),
        vec![SessionEvent::SessionChanged {
            from: Some(race),
            to: race
        }]
    );
    assert_eq!(n.stats().et_regressions, 1);
}

#[test]
fn withdrawn_standings_are_explicit_and_not_rebuilt_per_snapshot() {
    let mut fx = Fixture::driving(100.0);
    fx.rows[2].id = 3; // duplicate mID
    let mut n = Normalizer::new();
    let first = fx.step(&mut n);
    assert!(first.standings_withdrawn && first.scoring.is_none());
    for _ in 0..50 {
        fx.advance(0.01);
        let out = n.process_input(&fx.input(false));
        assert!(!out.standings_withdrawn && out.scoring.is_none());
    }
    assert_eq!(
        n.stats().scoring_updates,
        1,
        "rebuilt only on a scoring update"
    );
    assert_eq!(n.stats().scoring_rejected, 1, "counted per scoring update");
    fx.rows[2].id = 9;
    fx.advance(0.01);
    let fixed = fx.step(&mut n);
    assert!(!fixed.standings_withdrawn && fixed.scoring.is_some());
}

#[test]
fn zero_tyre_wear_is_unavailable_never_a_valid_zero() {
    let mut fx = Fixture::driving(100.0);
    for w in &mut fx.t().wheels {
        w.wear = 0.0;
    }
    let mut n = Normalizer::new();
    let mut last = fx.step(&mut n);
    for _ in 0..(2 * CAPABILITY_DECISION_SAMPLES) {
        fx.advance(0.01);
        last = fx.step(&mut n);
    }
    assert_eq!(n.vehicle_capabilities().supports_tyre_wear, None);
    for wheel in Wheel::ALL {
        assert_eq!(
            q(&last, wheel_id(ch::TYRE_WEAR_FL, wheel)),
            Some(Quality::Unavailable)
        );
    }
    fx.advance(0.01);
    fx.t().wheels[3].wear = 0.02;
    let worn = fx.step(&mut n);
    assert_eq!(n.vehicle_capabilities().supports_tyre_wear, Some(true));
    assert!(close(f(&worn, ch::TYRE_WEAR_RR), 0.02));
    assert_eq!(q(&worn, ch::TYRE_WEAR_FL), Some(Quality::Valid));
}

#[test]
fn session_time_remaining_needs_a_real_end_time() {
    use crate::mapping::session_time_remaining;
    assert_eq!(session_time_remaining(3_600.0, 100.0), Some(3_500.0));
    assert_eq!(
        session_time_remaining(3_600.0, 4_000.0),
        Some(0.0),
        "ran out"
    );
    assert_eq!(
        session_time_remaining(0.0, 100.0),
        None,
        "lap race / no end"
    );
    assert_eq!(session_time_remaining(-1.0, 100.0), None);
    assert_eq!(session_time_remaining(f64::NAN, 100.0), None);
    let mut fx = Fixture::driving(100.0);
    fx.info.end_et = 0.0;
    let out = fx.step(&mut Normalizer::new());
    assert_eq!(
        q(&out, ch::SESSION_TIME_REMAINING_CALC),
        Some(Quality::Unavailable),
        "never a fabricated 0:00:00"
    );
}

#[test]
fn unclassified_position_is_no_data_not_invalid() {
    let mut fx = Fixture::driving(100.0);
    fx.rows[1].place = 0; // loading
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert_eq!(q(&out, ch::POSITION), Some(Quality::Unavailable));
    assert!(!out.rejections.iter().any(|(id, _)| *id == ch::POSITION));
}

#[test]
fn sustained_rejection_raises_one_diagnostic() {
    let mut fx = Fixture::driving(100.0);
    fx.t().engine_water_temp = 900.0; // persistently above the plausibility bound (unit mix-up)
    let mut n = Normalizer::new();
    let mut raised = Vec::new();
    for i in 1..=(2 * SUSTAINED_REJECTION_FRAMES) {
        let out = fx.step(&mut n);
        assert_eq!(q(&out, ch::WATER_TEMP), Some(Quality::Invalid));
        raised.extend(
            out.notices
                .into_iter()
                .filter(|x| x.code == NoticeCode::ValueRejectedSustained)
                .map(|x| (i, x)),
        );
        fx.advance(0.01);
    }
    assert_eq!(raised.len(), 1, "deduplicated");
    let (at, notice) = &raised[0];
    assert_eq!(*at, SUSTAINED_REJECTION_FRAMES);
    assert_eq!(notice.channel, Some(ch::WATER_TEMP));
    assert_eq!(notice.raw_value, Some(900.0));

    // An intermittent rejection never accumulates.
    let mut fx = Fixture::driving(100.0);
    let mut n = Normalizer::new();
    for i in 0..(3 * SUSTAINED_REJECTION_FRAMES) {
        fx.t().engine_water_temp = if i % 2 == 0 { 900.0 } else { 85.0 };
        let out = fx.step(&mut n);
        assert!(
            !out.notices
                .iter()
                .any(|x| x.code == NoticeCode::ValueRejectedSustained)
        );
        fx.advance(0.01);
    }
}

#[test]
fn kelvin_brakes_are_published_only_once_decided_after_a_realtime_entry() {
    let cold = |fx: &mut Fixture| {
        fx.t().local_vel.z = 0.0;
        for w in &mut fx.t().wheels {
            w.brake_temp = 296.0; // ≈ 22.85 °C if Kelvin, ambient 22 °C
        }
    };
    // App running before the car goes on track: monitor (not realtime), then the realtime entry.
    let mut fx = Fixture::driving(10.0);
    fx.generic.game_version = UNVERIFIED_FUTURE_VERSION;
    cold(&mut fx);
    fx.info.in_realtime = false;
    let mut n = Normalizer::new();
    fx.step(&mut n);
    fx.info.in_realtime = true;
    let mut last = None;
    for _ in 0..BRAKE_UNIT_VOTES_REQUIRED {
        fx.advance(0.01);
        let out = fx.step(&mut n);
        last = Some(out);
    }
    let last = last.expect("steps");
    assert_eq!(
        (n.brake_temp_unit().state, n.brake_temp_unit().unit),
        (UnitDecisionState::Decided, BrakeTempUnit::Kelvin)
    );
    assert!(close(f(&last, ch::BRAKE_TEMP_FL), 22.85));
    assert_eq!(q(&last, ch::BRAKE_TEMP_RR), Some(Quality::Valid));

    // App started mid-session with the car already in realtime: no window, nothing published.
    let mut late = Fixture::driving(2_000.0);
    late.generic.game_version = UNVERIFIED_FUTURE_VERSION;
    cold(&mut late);
    let mut n = Normalizer::new();
    let mut out = late.step(&mut n);
    for _ in 0..(5 * BRAKE_UNIT_VOTES_REQUIRED) {
        late.advance(0.01);
        out = late.step(&mut n);
    }
    assert_eq!(n.brake_temp_unit().state, UnitDecisionState::Assumed);
    assert_eq!(q(&out, ch::BRAKE_TEMP_FL), Some(Quality::Unavailable));
}

#[test]
fn ambiguous_energy_scale_is_withheld_not_published_as_a_fraction() {
    let mut fx = Fixture::driving(100.0);
    // Evidence-only path: a version without a verified VE scale.
    fx.generic.game_version = UNVERIFIED_FUTURE_VERSION;
    fx.t().virtual_energy = 0.9; // 0.9 % or 90 %?
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert_eq!(q(&out, ch::VIRTUAL_ENERGY), Some(Quality::Unavailable));
    assert!(
        out.notices
            .iter()
            .any(|x| x.code == NoticeCode::FractionScaleUndecided
                && x.channel == Some(ch::VIRTUAL_ENERGY))
    );
    assert_eq!(n.virtual_energy_scale(), FractionScale::Undecided);
    // Two laps of consumption in 0..1.5 confirm 0..1.
    for (lap, ve) in [(4, 0.85), (5, 0.8)] {
        fx.advance(0.01);
        fx.t().lap_number = lap;
        fx.t().virtual_energy = ve;
        fx.step(&mut n);
    }
    assert_eq!(n.virtual_energy_scale(), FractionScale::Fraction);
    fx.advance(0.01);
    let confirmed = fx.step(&mut n);
    assert!(close(f(&confirmed, ch::VIRTUAL_ENERGY), 0.8));
}

/// `mCurrentSector` bit 31: the car is in the pit lane (A-LMU-026).
const PIT_LANE_BIT: i32 = i32::MIN;

/// A hybrid Hypercar-like driving fixture (LMU 1.4.2.0) with virtual energy in use.
fn hypercar(et: f64) -> Fixture {
    let mut fx = Fixture::driving(et);
    let t = fx.t();
    t.vehicle_class = 0;
    t.electric_boost_motor_state = 2;
    t.electric_boost_motor_torque = 150.0;
    t.electric_boost_motor_rpm = 12_000.0;
    t.electric_boost_motor_temperature = 70.0;
    t.soc = 60.0;
    t.battery_charge_fraction = 0.6;
    t.virtual_energy = 0.95;
    fx
}

/// Channels rejected by validation in `out`, by name.
fn rejected(out: &NormalizedOutput) -> Vec<&'static str> {
    out.rejections
        .iter()
        .filter_map(|(id, _)| registry::by_id(*id).map(|d| d.name))
        .collect()
}

#[test]
fn verified_version_publishes_virtual_energy_from_the_first_frame() {
    // Gate 1 (lmu-verify, 2026-10-12): LMU 1.4.2.0 reports mVirtualEnergy as a 0..1 fraction (A-LMU-013).
    let fx = hypercar(100.0);
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert!(close(f(&out, ch::VIRTUAL_ENERGY), 0.95), "no two-lap wait");
    assert_eq!(n.virtual_energy_scale(), FractionScale::Fraction);
    assert!(
        !out.notices
            .iter()
            .any(|x| x.code == NoticeCode::FractionScaleUndecided)
    );
    let note = n
        .capability_map()
        .get(ch::VIRTUAL_ENERGY)
        .and_then(|c| c.note.clone());
    assert!(note.is_some_and(|s| s.contains("verified with real data")));
}

#[test]
fn percent_virtual_energy_on_a_verified_version_is_withheld() {
    let mut fx = hypercar(100.0);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    // A reading above 1.5 contradicts the verified 0..1 scale: LMU changed it, nothing is guessed.
    fx.advance(0.01);
    fx.t().virtual_energy = 87.0;
    let out = fx.step(&mut n);
    assert_eq!(q(&out, ch::VIRTUAL_ENERGY), Some(Quality::Unavailable));
    assert_eq!(n.virtual_energy_scale(), FractionScale::Conflicting);
    assert!(
        out.notices
            .iter()
            .any(|x| x.code == NoticeCode::FractionScaleConflict
                && x.channel == Some(ch::VIRTUAL_ENERGY))
    );
    fx.advance(0.01);
    fx.t().virtual_energy = 0.9;
    let later = fx.step(&mut n);
    assert_eq!(
        q(&later, ch::VIRTUAL_ENERGY),
        Some(Quality::Unavailable),
        "withheld for the session"
    );
}

#[test]
fn real_extremes_seen_at_gate1_are_valid() {
    // Values the Gate-1 capture (LMU 1.4.2.0, Hypercar) showed in real driving; all were INVALID before.
    let mut fx = hypercar(100.0);
    fx.rows[1].pit_state = 5; // garage / leaving the garage (A-LMU-063)
    {
        let t = fx.t();
        t.electric_boost_motor_state = 4; // after a garage release (A-LMU-064)
        t.electric_boost_motor_torque = -3_675.7; // regeneration
        t.clutch_rpm = -4_259.4; // sliding backwards in a spin
        t.wheels[0].toe = -0.3439; // front toe includes the steering angle (A-LMU-068)
        t.wheels[1].toe = 0.3464;
        t.wheels[2].ride_height = 0.9; // a wheel off the ground in a crash
        t.wheels[3].temperature = [461.55, 353.15, 273.15]; // locked rear tyre: +188.4 °C spread
    }
    let out = fx.step(&mut Normalizer::new());
    assert!(
        out.rejections.is_empty(),
        "rejections: {:?}",
        rejected(&out)
    );
    let frame = out.frame.as_ref().expect("frame");
    assert_eq!(frame.get(ch::PIT_STATE).value, Value::U8(5));
    assert_eq!(frame.get(ch::HYBRID_MOTOR_STATE).value, Value::U8(4));
    assert!(close(f(&out, ch::CLUTCH_RPM), -4_259.4));
    assert!(close(f(&out, ch::HYBRID_MOTOR_TORQUE), -3_675.7));
    assert!(close(f(&out, ch::TOE_FR), 0.3464));
    assert!(close(f(&out, ch::RIDE_HEIGHT_RL), 0.9));
    assert!(close(f(&out, ch::TYRE_TEMP_SPREAD_RR), 188.4));
}

#[test]
fn garage_teleport_resets_rate_guards_instead_of_rejecting() {
    let mut fx = hypercar(397.0);
    let mut n = Normalizer::new();
    assert!(fx.step(&mut n).rejections.is_empty());
    // Return to the garage (Gate 1: 652 m in 7.5 ms, at rest, pit-lane bit set): position, speed, fuel,
    // brakes, tyres and energy all step in one frame.
    fx.advance(0.0075);
    {
        let t = fx.t();
        t.pos = crate::testutil::v3(307.2, 3.6, 575.1);
        t.local_vel = crate::testutil::v3(0.0, 0.0, -0.11);
        t.current_sector = PIT_LANE_BIT | 1;
        t.fuel = 81.0;
        t.virtual_energy = 1.0;
        t.electric_boost_motor_temperature = 23.5;
        for w in &mut t.wheels {
            w.brake_temp = 295.0;
            w.wear = 1.0;
            w.tire_inner_layer_temperature = [300.0; 3];
        }
    }
    let out = fx.step(&mut n);
    assert!(
        out.rejections.is_empty(),
        "rejections: {:?}",
        rejected(&out)
    );
    assert_eq!(n.stats().teleports, 1);
    assert!(close(f(&out, ch::POS_Z), 575.1));
    assert!(close(f(&out, ch::FUEL), 81.0));
    assert!(close(f(&out, ch::BRAKE_TEMP_FL), 295.0 - 273.15));
    // After the teleport the guards judge the new values again: driving off, a fuel spike is rejected.
    fx.advance(0.01);
    fx.t().current_sector = 1;
    fx.t().local_vel = crate::testutil::v3(0.0, 0.0, -0.5);
    fx.t().fuel = 10.0;
    let spike = fx.step(&mut n);
    assert_eq!(rejected(&spike), vec!["fuel.level_l"]);
}

#[test]
fn uncorroborated_jumps_are_still_rejected() {
    // The same 652 m jump out on track (no pit-lane bit, no pit flags): a garbage position.
    let mut fx = hypercar(100.0);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    fx.advance(0.01);
    fx.t().pos = crate::testutil::v3(307.2, 3.6, 575.1);
    fx.t().local_vel = crate::testutil::v3(0.0, 0.0, -0.11);
    let out = fx.step(&mut n);
    let names = rejected(&out);
    assert!(names.contains(&"vehicle.pos_x_m"), "{names:?}");
    assert_eq!(n.stats().teleports, 0);

    // In the pit lane but still moving at 50 m/s after the jump: not a teleport either.
    let mut moving = hypercar(100.0);
    let mut m = Normalizer::new();
    moving.step(&mut m);
    moving.advance(0.01);
    moving.t().pos = crate::testutil::v3(307.2, 3.6, 575.1);
    moving.t().current_sector = PIT_LANE_BIT | 1;
    let out = moving.step(&mut m);
    assert!(rejected(&out).contains(&"vehicle.pos_z_m"));

    // A fuel or tyre step while driving on track is still implausible.
    let mut track = hypercar(100.0);
    let mut k = Normalizer::new();
    track.step(&mut k);
    track.advance(0.01);
    track.t().fuel = 90.0;
    track.t().wheels[0].wear = 1.0;
    let names = rejected(&track.step(&mut k));
    assert!(names.contains(&"fuel.level_l") && names.contains(&"tyres.fl.wear_frac"));
}

#[test]
fn pit_service_steps_are_accepted_while_stationary_in_the_pits() {
    let mut fx = hypercar(200.0);
    fx.t().local_vel = crate::testutil::v3(0.0, 0.0, 0.0);
    fx.rows[1].in_pits = true;
    fx.rows[1].pit_state = 3;
    let mut n = Normalizer::new();
    assert!(fx.step(&mut n).rejections.is_empty());
    // Refuel and new tyres in one frame (as LMU applies them in the garage), car on its jacks.
    fx.advance(0.01);
    {
        let t = fx.t();
        t.fuel = 90.0;
        t.virtual_energy = 1.0;
        for w in &mut t.wheels {
            w.wear = 1.0;
            w.tire_inner_layer_temperature = [300.0; 3];
            w.tire_carcass_temperature = 300.0;
            w.pressure = 140.0;
        }
        // Brakes are not serviced: a 373 °C drop in 10 ms is still implausible.
        t.wheels[0].brake_temp = 300.0;
    }
    let out = fx.step(&mut n);
    assert_eq!(rejected(&out), vec!["brakes.fl.temp_c"]);
    assert!(close(f(&out, ch::FUEL), 90.0));
    assert!(close(f(&out, ch::TYRE_WEAR_RR), 1.0));
    assert!(n.stats().pit_service_samples >= 2);

    // The same steps stationary on track (no pit flag, no pit-lane bit) are rejected.
    let mut track = hypercar(200.0);
    track.t().local_vel = crate::testutil::v3(0.0, 0.0, 0.0);
    let mut k = Normalizer::new();
    track.step(&mut k);
    track.advance(0.01);
    track.t().fuel = 90.0;
    assert_eq!(rejected(&track.step(&mut k)), vec!["fuel.level_l"]);
}

#[test]
fn realtime_entry_resets_rate_guards() {
    let mut fx = hypercar(100.0);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    // Back to the monitor, then put on track again: the car state is re-initialised.
    fx.advance(0.01);
    fx.info.in_realtime = false;
    let player = fx.telemetry.take();
    n.process_input(&fx.input(true));
    fx.telemetry = player;
    fx.info.in_realtime = true;
    fx.advance(0.01);
    fx.t().fuel = 95.0;
    let out = fx.step(&mut n);
    assert!(
        out.rejections.is_empty(),
        "rejections: {:?}",
        rejected(&out)
    );
    assert_eq!(n.stats().realtime_entries, 1);
}

#[test]
fn channels_lmu_does_not_fill_are_withheld() {
    // Gate 1 (LMU 1.4.2.0, Hypercar): ride heights, wing height, drag and downforce exactly 0 and the turbo
    // boost exactly 101,325 Pa for the whole session while the car lapped (A-LMU-065).
    let mut fx = hypercar(100.0);
    fx.t().turbo_boost_pressure = 101_325.0;
    fx.t().local_vel = crate::testutil::v3(0.0, 0.0, 0.0);
    let mut n = Normalizer::new();
    let rest = fx.step(&mut n);
    // At rest: 0 N of drag and ambient boost are physically possible, a 0 m ride height and a front wing on
    // the ground are not.
    assert!(close(f(&rest, ch::DRAG), 0.0));
    assert!(close(f(&rest, ch::TURBO_BOOST), 101_325.0));
    assert_eq!(q(&rest, ch::RIDE_HEIGHT_FRONT), Some(Quality::Unavailable));
    assert_eq!(q(&rest, ch::FRONT_WING_HEIGHT), Some(Quality::Unavailable));
    // Moving off with the values unchanged: exactly 0 N of drag or ISA boost under way is not a measurement.
    fx.advance(0.01);
    fx.t().local_vel = crate::testutil::v3(0.0, 0.0, -16.0);
    let moving = fx.step(&mut n);
    let ids = [
        ch::DRAG,
        ch::DOWNFORCE_FRONT,
        ch::DOWNFORCE_REAR,
        ch::TURBO_BOOST,
        ch::RIDE_HEIGHT_FRONT,
        ch::RIDE_HEIGHT_REAR,
        ch::FRONT_WING_HEIGHT,
    ];
    for id in ids {
        assert_eq!(q(&moving, id), Some(Quality::Unavailable), "{id:?}");
    }
    let raised: Vec<ChannelId> = rest
        .notices
        .iter()
        .chain(moving.notices.iter())
        .filter(|x| x.code == NoticeCode::ChannelNotFilled)
        .filter_map(|x| x.channel)
        .collect();
    assert!(ids.iter().all(|id| raised.contains(id)), "{raised:?}");
    // After 200 m of driving (60 × 0.25 s at 16 m/s = 240 m) the capability map says why.
    for _ in 0..60 {
        fx.advance(0.25);
        fx.step(&mut n);
    }
    for id in ids {
        let note = n.capability_map().get(id).and_then(|c| c.note.clone());
        assert!(note.is_some_and(|s| s.contains("not filled")), "{id:?}");
    }
    // The moment LMU writes a new value the channel is live again (and stays live).
    fx.advance(0.01);
    fx.t().drag = 812.5;
    let live = fx.step(&mut n);
    assert!(close(f(&live, ch::DRAG), 812.5));
    assert_eq!(q(&live, ch::DOWNFORCE_FRONT), Some(Quality::Unavailable));
    fx.advance(0.01);
    fx.t().drag = 0.0;
    assert!(close(f(&fx.step(&mut n), ch::DRAG), 0.0), "latched live");
}

#[test]
fn negative_lap_distance_before_the_first_crossing_is_valid() {
    // Gate 1 race (Sebring): −1,586.8 m in the garage stall, −1,023.4 m on the grid (A-LMU-070).
    let mut fx = Fixture::driving(100.0);
    fx.rows[1].lap_dist = -1_586.8;
    fx.rows[1].total_laps = 0;
    let out = fx.step(&mut Normalizer::new());
    assert!(close(f(&out, ch::LAP_DISTANCE), -1_586.8));
    assert!(close(f(&out, ch::DISTANCE), -1_586.8));
    let progress = f(&out, ch::LAP_PROGRESS);
    assert!(
        progress.is_some_and(|p| (0.8..1.0).contains(&p)),
        "{progress:?}"
    );
    // More than one lap behind the line is not a position on this track.
    let mut bad = Fixture::driving(100.0);
    bad.rows[1].lap_dist = -bad.info.lap_dist - 500.0;
    let out = bad.step(&mut Normalizer::new());
    assert_eq!(q(&out, ch::LAP_DISTANCE), Some(Quality::Invalid));
    assert_eq!(q(&out, ch::DISTANCE), Some(Quality::Unavailable));
}

#[test]
fn race_end_time_sentinel_is_unavailable() {
    // Gate 1 race: mEndET = INT32_MIN during the formation lap of a timed race.
    let mut fx = Fixture::driving(100.0);
    fx.info.end_et = f64::from(i32::MIN);
    let out = fx.step(&mut Normalizer::new());
    assert_eq!(q(&out, ch::SESSION_END_TIME), Some(Quality::Unavailable));
    assert!(
        out.rejections
            .iter()
            .all(|(id, _)| *id != ch::SESSION_END_TIME)
    );
}

#[test]
fn lmgt3_zero_tyre_forces_and_torque_are_withheld_until_filled() {
    // Gate 1 (LMU 1.4.2.0, LMGT3, race at Sebring and practice at Laguna Seca): tyre load, lateral and
    // longitudinal force, sliding fraction and engine torque read exactly 0 all session (A-LMU-069).
    let mut fx = hypercar(100.0);
    {
        let t = fx.t();
        t.engine_torque = 0.0;
        for w in &mut t.wheels {
            (
                w.tire_load,
                w.lateral_force,
                w.longitudinal_force,
                w.grip_fract,
            ) = (0.0, 0.0, 0.0, 0.0);
        }
        t.local_vel = crate::testutil::v3(0.0, 0.0, -16.0);
    }
    let mut n = Normalizer::new();
    let first = fx.step(&mut n);
    // A bit-exact, unchanged 0 on a moving car is impossible at once: never a VALID 0 (gate1c review).
    for id in [
        ch::TYRE_LOAD_RR,
        ch::TYRE_LAT_FORCE_FL,
        ch::TYRE_LONG_FORCE_FR,
        ch::TYRE_SLIDING_RL,
        ch::ENGINE_TORQUE,
        ch::ENGINE_POWER,
    ] {
        assert_eq!(q(&first, id), Some(Quality::Unavailable), "{id:?}");
    }
    for _ in 0..60 {
        fx.advance(0.25);
        fx.step(&mut n);
    }
    fx.advance(0.01);
    let stuck = fx.step(&mut n);
    for id in [
        ch::TYRE_LOAD_FL,
        ch::TYRE_LAT_FORCE_FR,
        ch::TYRE_LONG_FORCE_RL,
        ch::TYRE_SLIDING_RR,
        ch::ENGINE_TORQUE,
        ch::ENGINE_POWER,
    ] {
        assert_eq!(q(&stuck, id), Some(Quality::Unavailable), "{id:?}");
    }
    // The first non-zero value makes the channel VALID again.
    fx.advance(0.01);
    fx.t().wheels[0].tire_load = 4_200.0;
    fx.t().engine_torque = 310.0;
    let live = fx.step(&mut n);
    assert!(close(f(&live, ch::TYRE_LOAD_FL), 4_200.0));
    assert!(close(f(&live, ch::ENGINE_TORQUE), 310.0));
    assert_eq!(q(&live, ch::ENGINE_POWER), Some(Quality::Valid));
    assert_eq!(q(&live, ch::TYRE_LOAD_FR), Some(Quality::Unavailable));
}

#[test]
fn session_reset_does_not_republish_lmgt3_zeros_as_valid() {
    // The session detectors restart on an ET regression; LMGT3's unfilled zeros must stay withheld.
    let mut fx = hypercar(500.0);
    {
        let t = fx.t();
        t.engine_torque = 0.0;
        for w in &mut t.wheels {
            (w.lateral_force, w.longitudinal_force, w.grip_fract) = (0.0, 0.0, 0.0);
        }
        t.local_vel = crate::testutil::v3(0.0, 0.0, -16.0);
    }
    let mut n = Normalizer::new();
    fx.step(&mut n);
    fx.advance(-400.0);
    let out = fx.step(&mut n);
    assert_eq!(
        out.disposition,
        TelemetryDisposition::NewSampleAfterRegression
    );
    for id in [
        ch::TYRE_LAT_FORCE_RR,
        ch::TYRE_LONG_FORCE_FL,
        ch::TYRE_SLIDING_FR,
        ch::ENGINE_TORQUE,
        ch::ENGINE_POWER,
    ] {
        assert_eq!(q(&out, id), Some(Quality::Unavailable), "{id:?}");
    }
}

/// Second normalizer review F2 (verify-v5 race1/pitstop1): at rest in the pit box, at the session start and
/// after a reset, LMGT3's unfilled zeros were published VALID 0. They stay withheld until the first change.
#[test]
fn lmgt3_zeros_at_rest_in_the_pit_box_are_withheld_after_a_reset() {
    let mut fx = hypercar(500.0);
    {
        let t = fx.t();
        t.engine_torque = 0.0;
        for w in &mut t.wheels {
            (
                w.tire_load,
                w.lateral_force,
                w.longitudinal_force,
                w.grip_fract,
            ) = (0.0, 0.0, 0.0, 0.0);
        }
        t.local_vel = crate::testutil::v3(0.0, 0.0, 0.0);
        t.current_sector = PIT_LANE_BIT | 1;
    }
    let ids = [
        ch::TYRE_LOAD_FL,
        ch::TYRE_LAT_FORCE_RR,
        ch::TYRE_LONG_FORCE_FL,
        ch::TYRE_SLIDING_FR,
        ch::ENGINE_TORQUE,
        ch::ENGINE_POWER,
    ];
    let mut n = Normalizer::new();
    let start = fx.step(&mut n);
    for id in ids {
        assert_eq!(q(&start, id), Some(Quality::Unavailable), "start {id:?}");
    }
    fx.advance(-400.0);
    let out = fx.step(&mut n);
    assert_eq!(
        out.disposition,
        TelemetryDisposition::NewSampleAfterRegression
    );
    for id in ids {
        assert_eq!(q(&out, id), Some(Quality::Unavailable), "reset {id:?}");
    }
    // Still at rest, but the value now changed: it is a measurement.
    fx.advance(0.01);
    fx.t().wheels[0].tire_load = 4_100.0;
    let live = fx.step(&mut n);
    assert!(close(f(&live, ch::TYRE_LOAD_FL), 4_100.0));
    assert_eq!(q(&live, ch::ENGINE_TORQUE), Some(Quality::Unavailable));
}

#[test]
fn a_stuck_plausible_value_is_withheld_after_the_travel_evidence() {
    // A constant that would be plausible on its own (1,234.5 N of drag) but never changes while the car
    // drives is still not a measurement once 200 m are behind it.
    let mut fx = hypercar(100.0);
    fx.t().drag = 1_234.5;
    let mut n = Normalizer::new();
    let first = fx.step(&mut n);
    assert!(close(f(&first, ch::DRAG), 1_234.5), "no evidence yet");
    let mut last = first;
    // 50 m/s × 0.25 s = 12.5 m per step: 16 steps reach exactly 200 m.
    for _ in 0..16 {
        fx.advance(0.25);
        last = fx.step(&mut n);
    }
    assert_eq!(q(&last, ch::DRAG), Some(Quality::Unavailable));
    assert!(
        last.notices
            .iter()
            .any(|x| x.code == NoticeCode::ChannelNotFilled && x.channel == Some(ch::DRAG))
    );
}

#[test]
fn channels_lmu_fills_are_never_withheld() {
    let mut fx = hypercar(100.0);
    let mut n = Normalizer::new();
    let mut last = None;
    for i in 0..40 {
        let t = fx.t();
        t.drag = 4_000.0 + f64::from(i);
        t.front_ride_height = 0.045 - f64::from(i) * 1e-4;
        t.turbo_boost_pressure = 180_000.0 + f64::from(i);
        last = Some(fx.step(&mut n));
        fx.advance(0.25);
    }
    let last = last.expect("steps");
    for id in [ch::DRAG, ch::RIDE_HEIGHT_FRONT, ch::TURBO_BOOST] {
        assert_eq!(q(&last, id), Some(Quality::Valid), "{id:?}");
    }
}

#[test]
fn solo_session_gaps_are_no_data_not_zero() {
    // Gate 1 (solo practice, one car): the four gaps read exactly 0 all session; there is no car to measure
    // a gap to (A-LMU-066).
    let mut fx = hypercar(100.0);
    fx.info.num_vehicles = 1;
    fx.rows = vec![row(PLAYER_ID, 1, "Hyper", true)];
    let out = fx.step(&mut Normalizer::new());
    for id in [
        ch::GAP_CAR_AHEAD,
        ch::GAP_CAR_BEHIND,
        ch::GAP_PLACE_AHEAD,
        ch::GAP_PLACE_BEHIND,
    ] {
        let s = out
            .frame
            .as_ref()
            .map(|fr| fr.get(id).clone())
            .expect("frame");
        assert_eq!(
            (s.quality, s.value),
            (Quality::Unavailable, Value::None),
            "{id:?}"
        );
    }
    // With other cars the gap is published as a magnitude: LMU signs "ahead" negative (Gate 1 race, A-LMU-034).
    let mut field = hypercar(100.0);
    field.t().time_gap_car_ahead = -1.25;
    field.t().time_gap_car_behind = 0.5;
    let out = field.step(&mut Normalizer::new());
    assert!(close(f(&out, ch::GAP_CAR_AHEAD), 1.25));
    assert!(close(f(&out, ch::GAP_CAR_BEHIND), 0.5));
}

/// A three-car field with the player at `place` (1-based) and every raw gap set to a plausible value.
fn three_car_field(place: u8) -> Fixture {
    let mut fx = hypercar(100.0);
    fx.info.num_vehicles = 3;
    let others = [3, 9];
    let mut other = others.iter();
    fx.rows = (1..=3u8)
        .map(|p| {
            if p == place {
                row(PLAYER_ID, p, "Hyper", true)
            } else {
                let id = other.next().copied().unwrap_or(11);
                row(id, p, "Hyper", false)
            }
        })
        .collect();
    let t = fx.t();
    (t.time_gap_car_ahead, t.time_gap_car_behind) = (-1.0, 2.0);
    (t.time_gap_place_ahead, t.time_gap_place_behind) = (-1.0, 2.0);
    fx
}

fn is_no_data(out: &NormalizedOutput, id: ChannelId) -> bool {
    out.frame.as_ref().is_some_and(|fr| {
        fr.get(id).quality == Quality::Unavailable && fr.get(id).value == Value::None
    })
}

#[test]
fn leader_and_last_car_place_gaps_are_no_data() {
    // gate1c review: P1 has no place ahead, the last car no place behind; the other side is published.
    let leader = three_car_field(1).step(&mut Normalizer::new());
    assert!(is_no_data(&leader, ch::GAP_PLACE_AHEAD));
    assert!(close(f(&leader, ch::GAP_PLACE_BEHIND), 2.0));
    let last = three_car_field(3).step(&mut Normalizer::new());
    assert!(is_no_data(&last, ch::GAP_PLACE_BEHIND));
    assert!(close(f(&last, ch::GAP_PLACE_AHEAD), 1.0));
    let middle = three_car_field(2).step(&mut Normalizer::new());
    assert!(close(f(&middle, ch::GAP_PLACE_AHEAD), 1.0));
    assert!(close(f(&middle, ch::GAP_PLACE_BEHIND), 2.0));
}

#[test]
fn wrong_sign_gaps_are_invalid_and_alongside_or_zero_gaps_are_plus_zero() {
    let mut fx = three_car_field(2);
    {
        let t = fx.t();
        t.time_gap_place_behind = -2.0; // clearly wrong sign: rejected, never a fabricated gap
        t.time_gap_car_ahead = 0.01; // alongside: LMU's wrong-sign jitter
        t.time_gap_car_behind = 0.0; // raw 0
    }
    let out = fx.step(&mut Normalizer::new());
    assert_eq!(q(&out, ch::GAP_PLACE_BEHIND), Some(Quality::Invalid));
    assert!(
        out.rejections
            .iter()
            .any(|(id, _)| *id == ch::GAP_PLACE_BEHIND),
        "{:?}",
        out.rejections
    );
    for id in [ch::GAP_CAR_AHEAD, ch::GAP_CAR_BEHIND] {
        assert_eq!(q(&out, id), Some(Quality::Valid), "{id:?}");
        let gap = f(&out, id);
        assert!(
            gap.is_some_and(|g| g == 0.0 && g.is_sign_positive()),
            "{id:?} {gap:?}"
        );
    }
    // A raw 0 ahead (negated) is +0.0 as well, never −0.0.
    fx.t().time_gap_car_ahead = 0.0;
    let out = fx.step(&mut Normalizer::new());
    assert!(f(&out, ch::GAP_CAR_AHEAD).is_some_and(|g| g == 0.0 && g.is_sign_positive()));
}

#[test]
fn verified_version_publishes_kelvin_brakes_from_the_first_frame() {
    // Gate 1 (2026-10-02): LMU 1.4.2.0 reports mBrakeTemp in Kelvin (cold = ambient + 273.15).
    let fx = Fixture::driving(500.0);
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert_eq!(
        (n.brake_temp_unit().state, n.brake_temp_unit().unit),
        (UnitDecisionState::Verified, BrakeTempUnit::Kelvin)
    );
    for wheel in Wheel::ALL {
        assert!(close(f(&out, wheel_id(ch::BRAKE_TEMP_FL, wheel)), 400.0));
    }
    assert!(
        !out.notices
            .iter()
            .any(|x| x.code == NoticeCode::BrakeTempUnitUnverified)
    );
    let note = n
        .capability_map()
        .get(ch::BRAKE_TEMP_FL)
        .and_then(|c| c.note.clone());
    assert!(note.is_some_and(|s| s.contains("verified with real data")));
}

#[test]
fn evidence_against_the_verified_unit_withholds_brake_temperatures() {
    // Readings that cannot be Kelvin (all four below 200 and non-zero) on a version verified as Kelvin:
    // LMU changed the unit, so nothing is published as VALID any more.
    let mut fx = Fixture::driving(500.0);
    for w in &mut fx.t().wheels {
        w.brake_temp = 150.0;
    }
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert_eq!(n.brake_temp_unit().state, UnitDecisionState::Conflicting);
    assert_eq!(q(&out, ch::BRAKE_TEMP_FL), Some(Quality::Unavailable));
    assert!(
        out.notices
            .iter()
            .any(|x| x.code == NoticeCode::BrakeTempUnitConflict)
    );
}

#[test]
fn detached_wheel_values_are_unavailable_not_frozen() {
    // Gate 1: a wheel that came off kept constant pressure and carcass values for minutes (A-LMU-061).
    let mut fx = Fixture::driving(500.0);
    let fr = Wheel::FrontRight.index();
    fx.t().wheels[fr].detached = true;
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert!(out.rejections.is_empty(), "{:?}", out.rejections);
    let frame = out.frame.as_ref().expect("frame");
    assert_eq!(
        frame.get(ch::WHEEL_DETACHED_FR).value,
        Value::Bool(true),
        "the detached flag itself is published"
    );
    for id in [
        ch::TYRE_PRESSURE_FR,
        ch::TYRE_TEMP_MIDDLE_FR,
        ch::TYRE_CARCASS_TEMP_FR,
        ch::TYRE_TEMP_INNER_FR,
        ch::TYRE_LOAD_FR,
        ch::WHEEL_SPEED_FR,
        ch::WHEEL_ROT_FR,
        ch::CAMBER_FR,
    ] {
        assert_eq!(q(&out, id), Some(Quality::Unavailable), "{id:?}");
    }
    // The other wheels are unaffected.
    assert!(close(f(&out, ch::TYRE_PRESSURE_FL), 170_000.0));
    assert_eq!(q(&out, ch::TYRE_TEMP_MIDDLE_RR), Some(Quality::Valid));
}

#[test]
fn uninitialised_tyre_and_engine_readings_are_unavailable_not_invalid() {
    // Gate 1: during a garage/reset transition LMU reported 0 K tyre temperatures, 0 kPa and 0 °C coolant and
    // oil on every wheel/engine (A-LMU-062). Those are "no data", not measurements and not rejections.
    let mut fx = Fixture::driving(500.0);
    for w in &mut fx.t().wheels {
        w.temperature = [0.0; 3];
        w.tire_inner_layer_temperature = [0.0; 3];
        w.tire_carcass_temperature = 0.0;
        w.pressure = 0.0;
    }
    fx.t().engine_water_temp = 0.0;
    fx.t().engine_oil_temp = 0.0;
    let mut n = Normalizer::new();
    let out = fx.step(&mut n);
    assert!(out.rejections.is_empty(), "{:?}", out.rejections);
    for wheel in Wheel::ALL {
        for base in [
            ch::TYRE_PRESSURE_FL,
            ch::TYRE_TEMP_LEFT_FL,
            ch::TYRE_LAYER_TEMP_RIGHT_FL,
            ch::TYRE_CARCASS_TEMP_FL,
            ch::TYRE_TEMP_AVG_FL,
            ch::TYRE_TEMP_SPREAD_FL,
        ] {
            let id = wheel_id(base, wheel);
            assert_eq!(q(&out, id), Some(Quality::Unavailable), "{id:?}");
        }
    }
    assert_eq!(q(&out, ch::WATER_TEMP), Some(Quality::Unavailable));
    assert_eq!(q(&out, ch::OIL_TEMP), Some(Quality::Unavailable));

    // A flat tyre with a real thermal state: 0 kPa is a measurement and is published.
    let mut flat = Fixture::driving(500.0);
    flat.t().wheels[0].pressure = 0.0;
    flat.t().wheels[0].flat = true;
    let mut n = Normalizer::new();
    let out = flat.step(&mut n);
    assert_eq!(q(&out, ch::TYRE_PRESSURE_FL), Some(Quality::Valid));
    assert!(close(f(&out, ch::TYRE_PRESSURE_FL), 0.0));
}

// ---------------------------------------------------------------------------------------------------------
// gate1d: stint1 practice findings (synthetic fixtures, never the capture)
// ---------------------------------------------------------------------------------------------------------

/// LMU `mSession` of "Practice 1".
const LMU_SESSION_PRACTICE_1: i32 = 1;

#[test]
fn place_gaps_and_classification_gaps_are_no_data_outside_a_race() {
    // stint1 practice: the place gap is the ROAD gap to the car classified (by best lap) next to the player,
    // positive when that car is behind on the road (+11.9..+123.8 s), never a classification gap.
    let mut fx = three_car_field(2);
    fx.info.session = LMU_SESSION_PRACTICE_1;
    {
        let t = fx.t();
        (t.time_gap_place_ahead, t.time_gap_place_behind) = (54.1, 2.0);
    }
    for r in &mut fx.rows {
        (r.time_behind_next, r.laps_behind_next) = (-11.9, 1);
        (r.time_behind_leader, r.laps_behind_leader) = (0.0, 1);
    }
    let out = fx.step(&mut Normalizer::new());
    for id in [
        ch::GAP_PLACE_AHEAD,
        ch::GAP_PLACE_BEHIND,
        ch::TIME_BEHIND_NEXT,
        ch::LAPS_BEHIND_NEXT,
        ch::TIME_BEHIND_LEADER,
        ch::LAPS_BEHIND_LEADER,
    ] {
        assert!(is_no_data(&out, id), "{id:?}");
    }
    // The car gaps are road gaps by definition and stay published in every session type.
    assert!(close(f(&out, ch::GAP_CAR_AHEAD), 1.0));
    assert!(close(f(&out, ch::GAP_CAR_BEHIND), 2.0));
    let standings = out.scoring.expect("standings");
    assert!(
        standings
            .vehicles
            .iter()
            .all(|v| v.time_behind_next_s.is_none()
                && v.laps_behind_next.is_none()
                && v.time_behind_leader_s.is_none()
                && v.laps_behind_leader.is_none())
    );
}

#[test]
fn classification_gaps_are_published_in_a_race() {
    let mut fx = three_car_field(2);
    for r in &mut fx.rows {
        (r.time_behind_next, r.laps_behind_next) = (1.0, 0);
        (r.time_behind_leader, r.laps_behind_leader) = (3.0, 0);
    }
    let out = fx.step(&mut Normalizer::new());
    assert!(close(f(&out, ch::GAP_PLACE_AHEAD), 1.0));
    assert!(close(f(&out, ch::GAP_PLACE_BEHIND), 2.0));
    assert!(close(f(&out, ch::TIME_BEHIND_NEXT), 1.0));
    assert!(close(f(&out, ch::TIME_BEHIND_LEADER), 3.0));
    let standings = out.scoring.expect("standings");
    assert!(
        standings
            .vehicles
            .iter()
            .all(|v| v.time_behind_next_s == Some(1.0) && v.laps_behind_leader == Some(0))
    );
}

#[test]
fn lap_start_after_the_et_is_no_lap_time() {
    let mut fx = Fixture::driving(606.02);
    fx.t().lap_start_et = 648.87; // stint1: lap start left over after the session-end rewind
    let out = fx.step(&mut Normalizer::new());
    assert!(is_no_data(&out, ch::LAP_TIME));
    assert!(mapping::current_lap_time_s(397.04, Some(397.054)).is_none());
    assert!(mapping::current_lap_time_s(397.04, None).is_none());
    assert_eq!(mapping::current_lap_time_s(400.0, Some(397.0)), Some(3.0));
    assert_eq!(mapping::current_lap_time_s(397.0, Some(397.0)), Some(0.0));
}

/// stint1 practice tail: session over at mEndET 605 s, back in the garage (out of realtime) at ET 653 s, then
/// LMU rewinds ET to 606.02 s before EndSession.
fn session_over_in_the_garage(et: f64) -> Fixture {
    let mut fx = Fixture::driving(et);
    fx.info.session = LMU_SESSION_PRACTICE_1;
    fx.info.end_et = 605.0;
    fx.info.game_phase = 8;
    fx.info.in_realtime = false;
    let t = fx.t();
    t.lap_start_et = 648.87;
    t.local_vel = lmu_reader::Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    fx
}

#[test]
fn session_end_rewind_resets_the_time_base_without_a_restart_event() {
    let mut n = Normalizer::new();
    let mut fx = session_over_in_the_garage(653.025);
    fx.step(&mut n);
    fx.t().elapsed_time = 606.02;
    fx.info.current_et = 606.0;
    let rewound = fx.step(&mut n);
    assert_eq!(
        rewound.disposition,
        TelemetryDisposition::NewSampleAfterRegression
    );
    assert!(session_changes(std::slice::from_ref(&rewound)).is_empty());
    assert!(
        rewound
            .notices
            .iter()
            .any(|x| x.code == NoticeCode::EtRegression && x.message.contains("session-end rewind")),
        "{:?}",
        rewound.notices
    );
    assert!(
        rewound.frame.is_some(),
        "the garage sample is still published"
    );
    assert!(
        is_no_data(&rewound, ch::LAP_TIME),
        "never a negative lap time"
    );
    let stats = n.stats();
    assert_eq!((stats.et_regressions, stats.session_end_rewinds), (1, 1));
    // The next sample continues from the rewound ET: no further regression.
    fx.advance(0.02);
    let next = fx.step(&mut n);
    assert_eq!(next.disposition, TelemetryDisposition::NewSample);
    assert_eq!(n.stats().et_regressions, 1);
}

#[test]
fn regression_while_the_session_runs_is_still_a_restart() {
    // The same step back with the session running (green flag) is a restart, reported as such.
    let mut n = Normalizer::new();
    let mut fx = session_over_in_the_garage(653.025);
    fx.info.game_phase = 5;
    fx.step(&mut n);
    fx.t().elapsed_time = 606.02;
    fx.info.current_et = 606.0;
    let out = fx.step(&mut n);
    assert_eq!(session_changes(std::slice::from_ref(&out)).len(), 1);
    assert_eq!(n.stats().session_end_rewinds, 0);
}

/// Synthetic: the car is moved 19.6 m into the garage on the sample whose ET steps back (A-LMU-031 kind (b);
/// the real order is in `real_pitstop1_garage_return_step_back_after_the_jump_is_not_a_restart`).
/// A teleport step-back is not a session restart: no reset, no SessionChanged, published ET stays monotonic.
#[test]
fn garage_teleport_with_an_et_step_back_is_not_a_restart() {
    let mut fx = hypercar(315.830);
    {
        let t = fx.t();
        t.pos = crate::testutil::v3(0.0, 0.0, 0.0);
        t.local_vel = crate::testutil::v3(0.0, 0.0, -0.2);
        t.current_sector = PIT_LANE_BIT | 1;
    }
    let mut n = Normalizer::new();
    fx.step(&mut n);
    let resets = n.stats().session_resets;
    fx.advance(-0.010);
    {
        let t = fx.t();
        t.pos = crate::testutil::v3(19.6, 0.0, 0.0);
        t.local_vel = crate::testutil::v3(0.0, 0.0, 0.0);
        t.fuel = 81.0;
    }
    let back = fx.step(&mut n);
    assert_eq!(back.disposition, TelemetryDisposition::Duplicate);
    assert!(back.frame.is_none(), "the step-back sample is dropped");
    // Still behind the previous ET (within the step-back window): dropped as well, not a restart.
    fx.advance(0.005);
    let inside = fx.step(&mut n);
    assert_eq!(inside.disposition, TelemetryDisposition::Duplicate);
    // Back past the old ET: published, the rate guards accept the teleported values.
    fx.advance(0.0125);
    let next = fx.step(&mut n);
    assert_eq!(next.disposition, TelemetryDisposition::NewSample);
    assert!(next.rejections.is_empty(), "{:?}", rejected(&next));
    assert!(close(f(&next, ch::FUEL), 81.0));
    assert!(session_changes(&[back, inside, next]).is_empty());
    let stats = n.stats();
    assert_eq!(stats.session_resets, resets, "no session reset");
    assert_eq!(stats.teleport_et_step_backs, 2);
    assert_eq!(stats.et_regressions, 2);
    assert_eq!(stats.teleports, 1);
}

/// One player sample of the Gate 1 pitstop1 garage return (LMU 1.4.2.0, capture 255.64-255.72 s): player ET,
/// x position relative to the pre-jump place (m), speed (m/s). Pit-lane bit set, `mInRealtime` true,
/// `mGamePhase` 8 throughout.
const PITSTOP1_GARAGE_RETURN: [(f64, f64, f64); 6] = [
    (315.7925, 0.0, 0.259),
    (315.8025, 0.0, 0.169),
    // The return to the garage: a 19.17 m jump with ET still advancing (+0.0279 s), at rest.
    (315.8304, 19.17, 0.000),
    // 1.3 ms later: ET back by 0.0104 s, same place.
    (315.8200, 19.17, 0.004),
    (315.8400, 19.17, 0.006),
    (315.8600, 19.17, 0.006),
];

/// Sets the player sample of the fixture to `(et, x, speed)` (in the pit lane).
fn set_pit_sample(fx: &mut Fixture, (et, x, speed): (f64, f64, f64)) {
    let dt = et - fx.t().elapsed_time;
    fx.advance(dt);
    let t = fx.t();
    t.pos = crate::testutil::v3(x, 0.0, 0.0);
    t.local_vel = crate::testutil::v3(0.0, 0.0, -speed);
    t.current_sector = PIT_LANE_BIT | 1;
}

/// Real-data regression (verify-v6 pitstop1 elapsed_time FAIL): the ET step-back arrives on the sample
/// AFTER the garage jump, not with it. It is a teleport step-back (dropped, no reset, no SessionChanged);
/// the genuine new session later (ET 325.02 -> 0) still restarts.
#[test]
fn real_pitstop1_garage_return_step_back_after_the_jump_is_not_a_restart() {
    let mut fx = hypercar(PITSTOP1_GARAGE_RETURN[0].0);
    fx.info.game_phase = 8;
    set_pit_sample(&mut fx, PITSTOP1_GARAGE_RETURN[0]);
    let mut n = Normalizer::new();
    let mut outs = vec![fx.step(&mut n)];
    let resets = n.stats().session_resets;
    let mut dispositions = Vec::new();
    for &s in &PITSTOP1_GARAGE_RETURN[1..] {
        set_pit_sample(&mut fx, s);
        let out = fx.step(&mut n);
        dispositions.push(out.disposition);
        outs.push(out);
    }
    assert_eq!(
        dispositions,
        [
            TelemetryDisposition::NewSample,
            TelemetryDisposition::NewSample,
            TelemetryDisposition::Duplicate,
            TelemetryDisposition::NewSample,
            TelemetryDisposition::NewSample,
        ]
    );
    assert!(outs[3].frame.is_none(), "the step-back sample is dropped");
    // outs[0] carries the initial SessionChanged (from: None).
    assert!(session_changes(&outs[1..]).is_empty(), "no false restart");
    let stats = n.stats();
    assert_eq!(stats.session_resets, resets, "no session reset");
    assert_eq!(stats.teleports, 1, "the jump is counted once");
    assert_eq!(stats.teleport_et_step_backs, 1);
    assert_eq!(stats.et_regressions, 1);

    // The genuine new session of the same capture (ET 325.020 -> 0.000) is still a restart.
    set_pit_sample(&mut fx, (325.020, 19.17, 0.0));
    fx.step(&mut n);
    set_pit_sample(&mut fx, (0.0, 19.17, 0.0));
    let restart = fx.step(&mut n);
    assert_eq!(
        restart.disposition,
        TelemetryDisposition::NewSampleAfterRegression
    );
    assert_eq!(n.stats().session_resets, resets + 1);
    assert_eq!(session_changes(&[restart]).len(), 1);
}

/// The real step-back is explained only right after the jump: with an ordinary sample between the jump and
/// the step-back, the step-back is not corroborated and takes the restart path.
#[test]
fn step_back_not_right_after_the_garage_jump_is_still_a_restart() {
    let mut fx = hypercar(PITSTOP1_GARAGE_RETURN[0].0);
    set_pit_sample(&mut fx, PITSTOP1_GARAGE_RETURN[1]);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    set_pit_sample(&mut fx, PITSTOP1_GARAGE_RETURN[2]);
    fx.step(&mut n);
    assert_eq!(n.stats().teleports, 1);
    set_pit_sample(&mut fx, (315.8350, 19.17, 0.0));
    assert_eq!(fx.step(&mut n).disposition, TelemetryDisposition::NewSample);
    let resets = n.stats().session_resets;
    set_pit_sample(&mut fx, PITSTOP1_GARAGE_RETURN[3]);
    assert_eq!(
        fx.step(&mut n).disposition,
        TelemetryDisposition::NewSampleAfterRegression
    );
    assert_eq!(n.stats().session_resets, resets + 1);
    assert_eq!(n.stats().teleport_et_step_backs, 0);
}

/// A step-back without a garage jump (or a larger one) still takes the restart path.
#[test]
fn small_et_step_back_without_a_teleport_is_still_a_restart() {
    let mut fx = hypercar(315.830);
    let mut n = Normalizer::new();
    fx.step(&mut n);
    let resets = n.stats().session_resets;
    fx.advance(-0.010);
    let out = fx.step(&mut n);
    assert_eq!(
        out.disposition,
        TelemetryDisposition::NewSampleAfterRegression
    );
    assert_eq!(n.stats().session_resets, resets + 1);
    assert_eq!(n.stats().teleport_et_step_backs, 0);
}

/// Second normalizer review F3 (verify-v5 Hypercar): `mSoC` read 0 (VALID 0 published) while
/// `mBatteryChargeFraction` was 0.0095; elsewhere `mSoC` = 100 × battery. The battery fraction wins (A-LMU-014).
#[test]
fn soc_disagreeing_with_the_battery_fraction_publishes_the_battery_fraction() {
    let mut fx = hypercar(100.0);
    let mut n = Normalizer::new();
    let agree = fx.step(&mut n);
    assert!(
        close(f(&agree, ch::BATTERY_SOC), 0.6),
        "percent scale decided"
    );
    assert!(
        !agree
            .notices
            .iter()
            .any(|x| x.code == NoticeCode::HybridSocDisagreement)
    );
    // The captured pattern: SoC 0 next to a small, non-zero battery fraction.
    fx.advance(1.0); // within the 1 /s energy rate bound
    fx.t().soc = 0.0;
    fx.t().battery_charge_fraction = 0.0095;
    let zero = fx.step(&mut n);
    assert!(
        close(f(&zero, ch::BATTERY_SOC), 0.0095),
        "never a VALID 0: {:?} {:?}",
        q(&zero, ch::BATTERY_SOC),
        rejected(&zero)
    );
    assert!(
        zero.notices
            .iter()
            .any(|x| x.code == NoticeCode::HybridSocDisagreement)
    );
    // A larger disagreement: the battery fraction wins as well.
    fx.advance(1.0); // within the 1 /s energy rate bound
    fx.t().soc = 30.0;
    fx.t().battery_charge_fraction = 0.5;
    assert!(close(f(&fx.step(&mut n), ch::BATTERY_SOC), 0.5));
    // Agreement within f32 rounding keeps the scaled SoC.
    fx.advance(1.0); // within the 1 /s energy rate bound
    fx.t().soc = 42.0;
    fx.t().battery_charge_fraction = 0.420_000_1;
    assert!(close(f(&fx.step(&mut n), ch::BATTERY_SOC), 0.42));
}

/// Second normalizer review F3 (verify-v5 Hypercar): `hybrid.motor_temp_c` had VALID 0 samples while its p1
/// was 40.7 °C. An exact 0 before the first non-zero reading is not initialised: UNAVAILABLE.
#[test]
fn hybrid_motor_temperature_zero_is_unavailable_until_initialised() {
    let mut fx = hypercar(100.0);
    fx.t().electric_boost_motor_temperature = 0.0;
    let mut n = Normalizer::new();
    let first = fx.step(&mut n);
    assert_eq!(q(&first, ch::HYBRID_MOTOR_TEMP), Some(Quality::Unavailable));
    fx.advance(0.01);
    fx.t().electric_boost_motor_temperature = 40.7;
    let live = fx.step(&mut n);
    assert!(close(f(&live, ch::HYBRID_MOTOR_TEMP), 40.7));
    // A Kelvin field reading a raw 0 later would be -273 °C: withheld as well.
    let mut k = hypercar(100.0);
    k.t().electric_boost_motor_temperature = 313.85;
    let mut nk = Normalizer::new();
    assert!(close(f(&k.step(&mut nk), ch::HYBRID_MOTOR_TEMP), 40.7));
    k.advance(0.01);
    k.t().electric_boost_motor_temperature = 0.0;
    assert_eq!(
        q(&k.step(&mut nk), ch::HYBRID_MOTOR_TEMP),
        Some(Quality::Unavailable)
    );
}
/// Second normalizer review F5: A-LMU-016 is VERIFIED (Gate 1 stint1); the tyre inner/outer notes must not
/// keep an "unverified" caveat.
#[test]
fn tyre_side_mapping_note_cites_the_gate1_verification() {
    let n = Normalizer::new();
    for id in [
        ch::TYRE_TEMP_INNER_FL,
        ch::TYRE_TEMP_OUTER_RR,
        ch::TYRE_TEMP_SPREAD_FR,
    ] {
        let note = n.capability_map().get(id).and_then(|c| c.note.clone());
        assert!(
            note.as_deref()
                .is_some_and(|s| s.contains("verified at Gate 1") && !s.contains("unverified")),
            "{id:?}: {note:?}"
        );
    }
}

/// Second normalizer review F4 through the normalizer: a hybrid car in the garage stall reading motor state 0
/// and SoC 0 keeps its hybrid channels UNAVAILABLE (undecided), never NOT_SUPPORTED.
#[test]
fn hybrid_in_the_garage_stall_is_never_reported_without_hybrid() {
    let mut fx = hypercar(100.0);
    fx.t().electric_boost_motor_state = 0;
    fx.t().soc = 0.0;
    fx.rows[1].in_garage_stall = true;
    let mut n = Normalizer::new();
    let mut last = None;
    for _ in 0..(CAPABILITY_DECISION_SAMPLES + 10) {
        last = Some(fx.step(&mut n));
        fx.advance(0.01);
    }
    let last = last.expect("steps");
    assert_eq!(n.vehicle_capabilities().supports_hybrid, None);
    assert_eq!(q(&last, ch::BATTERY_SOC), Some(Quality::Unavailable));
    assert_eq!(q(&last, ch::HYBRID_MOTOR_TEMP), Some(Quality::Unavailable));
}

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
    t.virtual_energy = 87.0; // percent
    t.rear_flap_activated = 1;
    t.turbo_boost_pressure = 180.0;
    (t.tc_slip_max, t.tc_slip, t.tc_cut_max, t.tc_cut) = (5, 2, 5, 1);
    (t.migration_max, t.migration, t.motor_map_max, t.motor_map) = (3, 1, 4, 2);
    (t.front_anti_sway_max, t.front_anti_sway) = (5, 3);
    (t.rear_anti_sway_max, t.rear_anti_sway) = (5, 2);
    t.front_tire_compound_name = text("Medium");
    t.rear_tire_compound_name = text("Medium");
    for w in &mut t.wheels {
        w.brake_temp = 24.0; // impossible as Kelvin: the unit is decided (Celsius) within the run
        w.terrain_name = text("ROAD");
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
    assert_eq!(
        q(&out, ch::BRAKE_TEMP_FL),
        Some(Quality::Unavailable),
        "unit undecided: withheld, never an unverified VALID conversion"
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
        Some(S::Unavailable),
        "unit undecided"
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

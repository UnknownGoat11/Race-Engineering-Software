//! Publication of the tyre, brake, track, race-classification, flag and driver-aid engine outputs into
//! their registry channels (BLUEPRINT §14 Q5: recorded, streamed, graphable, quality-tagged channels instead
//! of a JSON side channel; D-017, schema 1 open until v0.1.0).
//!
//! Same rules as [`super::publish`]:
//! - every channel in [`EXTENDED_CHANNELS`] is written on every call, so nothing lingers from an earlier frame;
//! - a missing value (missing or `Unavailable` input, or the engine output not passed) is `Unavailable`,
//!   never 0;
//! - a value outside the registry range is `Invalid`, never clamped;
//! - the output's formula id and provenance must be the channel's registry contract; a mismatch is written
//!   `Invalid` and reported in [`PublishReport::errors`].
//!
//! Threshold bands are published as codes [`BAND_LOW`], [`BAND_IN_RANGE`], [`BAND_HIGH`]; an unknown band (no
//! value, or no threshold configured) is `Unavailable`. The flag summary uses [`flag_summary_code`].
//!
//! Gaps are routed by their formula: a race-classification class gap (`gap_from_time_behind_leader`,
//! estimated) goes to `race.class_gap_*`, a best-lap gap of practice / qualifying (`gap_best_lap_difference`)
//! to `race.best_lap_gap_*` / `race.class_best_lap_gap_*`; the channels of the other classification are
//! `Unavailable`. The overall race gaps are LMU values that already have measured channels.

use telemetry_model::events::RaceFlag;
use telemetry_model::registry;
use telemetry_model::{ChannelId, Provenance, Sample, TelemetryFrame, Value, ValueType, ch};
use tracing::{debug, warn};

use super::{PublishError, PublishReport, check_contract};
use crate::aids::AidsSnapshot;
use crate::brake::{self, BrakeSnapshot, WheelBrakeState};
use crate::common::{Metric, formula};
use crate::race::{Gap, RaceSnapshot};
use crate::tyre::{self, Band, TyreSnapshot, WHEELS, Wheel, WheelTyreState};

/// Tracing component name.
const COMPONENT: &str = "race_engines::publish::extended";

/// Band code: below the low threshold.
pub const BAND_LOW: u8 = 0;
/// Band code: within the configured thresholds.
pub const BAND_IN_RANGE: u8 = 1;
/// Band code: above the high threshold.
pub const BAND_HIGH: u8 = 2;

/// Channels written by [`publish_extended`] (per-wheel groups in FL, FR, RL, RR order).
pub const EXTENDED_CHANNELS: &[ChannelId] = &[
    ch::TYRE_PRESSURE_DELTA_FL,
    ch::TYRE_PRESSURE_DELTA_FR,
    ch::TYRE_PRESSURE_DELTA_RL,
    ch::TYRE_PRESSURE_DELTA_RR,
    ch::TYRE_LAYER_TEMP_AVG_FL,
    ch::TYRE_LAYER_TEMP_AVG_FR,
    ch::TYRE_LAYER_TEMP_AVG_RL,
    ch::TYRE_LAYER_TEMP_AVG_RR,
    ch::TYRE_TEMP_DELTA_FL,
    ch::TYRE_TEMP_DELTA_FR,
    ch::TYRE_TEMP_DELTA_RL,
    ch::TYRE_TEMP_DELTA_RR,
    ch::TYRE_CARCASS_TEMP_DELTA_FL,
    ch::TYRE_CARCASS_TEMP_DELTA_FR,
    ch::TYRE_CARCASS_TEMP_DELTA_RL,
    ch::TYRE_CARCASS_TEMP_DELTA_RR,
    ch::TYRE_TEMP_PEAK_FL,
    ch::TYRE_TEMP_PEAK_FR,
    ch::TYRE_TEMP_PEAK_RL,
    ch::TYRE_TEMP_PEAK_RR,
    ch::TYRE_TEMP_ROLLING_AVG_FL,
    ch::TYRE_TEMP_ROLLING_AVG_FR,
    ch::TYRE_TEMP_ROLLING_AVG_RL,
    ch::TYRE_TEMP_ROLLING_AVG_RR,
    ch::TYRE_TEMP_INNER_MIDDLE_FL,
    ch::TYRE_TEMP_INNER_MIDDLE_FR,
    ch::TYRE_TEMP_INNER_MIDDLE_RL,
    ch::TYRE_TEMP_INNER_MIDDLE_RR,
    ch::TYRE_TEMP_MIDDLE_OUTER_FL,
    ch::TYRE_TEMP_MIDDLE_OUTER_FR,
    ch::TYRE_TEMP_MIDDLE_OUTER_RL,
    ch::TYRE_TEMP_MIDDLE_OUTER_RR,
    ch::TYRE_TEMP_BAND_FL,
    ch::TYRE_TEMP_BAND_FR,
    ch::TYRE_TEMP_BAND_RL,
    ch::TYRE_TEMP_BAND_RR,
    ch::TYRE_PRESSURE_BAND_FL,
    ch::TYRE_PRESSURE_BAND_FR,
    ch::TYRE_PRESSURE_BAND_RL,
    ch::TYRE_PRESSURE_BAND_RR,
    ch::TYRE_EDGE_SPREAD_BAND_FL,
    ch::TYRE_EDGE_SPREAD_BAND_FR,
    ch::TYRE_EDGE_SPREAD_BAND_RL,
    ch::TYRE_EDGE_SPREAD_BAND_RR,
    ch::TYRE_TEMP_BALANCE_FRONT_REAR,
    ch::TYRE_TEMP_BALANCE_LEFT_RIGHT,
    ch::TYRE_PRESSURE_BALANCE_FRONT_REAR,
    ch::TYRE_PRESSURE_BALANCE_LEFT_RIGHT,
    ch::BRAKE_TEMP_PEAK_FL,
    ch::BRAKE_TEMP_PEAK_FR,
    ch::BRAKE_TEMP_PEAK_RL,
    ch::BRAKE_TEMP_PEAK_RR,
    ch::BRAKE_TEMP_BAND_FL,
    ch::BRAKE_TEMP_BAND_FR,
    ch::BRAKE_TEMP_BAND_RL,
    ch::BRAKE_TEMP_BAND_RR,
    ch::BRAKE_TEMP_FRONT_REAR,
    ch::BRAKE_BIAS_FRONT,
    ch::MAP_LAP_DISTANCE,
    ch::CLASS_GAP_AHEAD,
    ch::CLASS_GAP_BEHIND,
    ch::CLASS_GAP_AHEAD_LAPS,
    ch::CLASS_GAP_BEHIND_LAPS,
    ch::BEST_LAP_GAP_AHEAD,
    ch::BEST_LAP_GAP_BEHIND,
    ch::CLASS_BEST_LAP_GAP_AHEAD,
    ch::CLASS_BEST_LAP_GAP_BEHIND,
    ch::FLAG_SUMMARY,
    ch::FORMATION_LAP,
    ch::TC_DUTY,
    ch::ABS_DUTY,
];

/// Engine outputs of one extended publication step. `None` writes the engine's channels `Unavailable`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExtendedOutputs<'a> {
    /// Tyre engine snapshot.
    pub tyre: Option<&'a TyreSnapshot>,
    /// Brake engine snapshot.
    pub brake: Option<&'a BrakeSnapshot>,
    /// Lap distance projected on the track map ([`crate::track::TrackMap::projected_distance`]); `None`
    /// when no validated map is loaded.
    pub map_lap_distance: Option<Metric<f64>>,
    /// Race engine snapshot.
    pub race: Option<&'a RaceSnapshot>,
    /// Driver-aid duty cycles.
    pub aids: Option<&'a AidsSnapshot>,
}

/// A value an extended output carries.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Num {
    /// A float (written to F32/F64 channels only).
    Float(f64),
    /// An integer (written to integer channels only).
    Int(i64),
    /// A boolean.
    Bool(bool),
}

impl Num {
    /// The numeric value for the range check (`None` for a boolean, which has no range).
    fn ranged(self) -> Option<f64> {
        match self {
            Num::Float(x) => Some(x),
            Num::Int(n) => Some(n as f64),
            Num::Bool(_) => None,
        }
    }

    /// The value in the channel's wire type; `None` when it cannot be represented without rounding,
    /// wrapping or reinterpretation.
    fn to_value(self, value_type: ValueType) -> Option<Value> {
        match (self, value_type) {
            (Num::Float(x), ValueType::F32) if x.is_finite() => Some(Value::F32(x as f32)),
            (Num::Float(x), ValueType::F64) if x.is_finite() => Some(Value::F64(x)),
            (Num::Int(n), ValueType::I32) => i32::try_from(n).ok().map(Value::I32),
            (Num::Int(n), ValueType::U8) => u8::try_from(n).ok().map(Value::U8),
            (Num::Int(n), ValueType::I8) => i8::try_from(n).ok().map(Value::I8),
            (Num::Int(n), ValueType::U32) => u32::try_from(n).ok().map(Value::U32),
            (Num::Bool(b), ValueType::Bool) => Some(Value::Bool(b)),
            _ => None,
        }
    }
}

/// One output to publish: its contract (formula, provenance) and its value.
#[derive(Debug, Clone, Copy)]
struct Out {
    formula: &'static str,
    provenance: Provenance,
    value: Option<Num>,
}

impl Out {
    fn float(m: Metric<f64>) -> Self {
        Self {
            formula: m.formula,
            provenance: m.provenance,
            value: m.value.map(Num::Float),
        }
    }

    fn unavailable() -> Self {
        Self {
            formula: "",
            provenance: Provenance::Derived,
            value: None,
        }
    }
}

/// Band code; `None` for [`Band::Unknown`] (no value or no threshold: there is no universal correct value).
pub const fn band_code(band: Band) -> Option<u8> {
    match band {
        Band::Low => Some(BAND_LOW),
        Band::InRange => Some(BAND_IN_RANGE),
        Band::High => Some(BAND_HIGH),
        Band::Unknown => None,
    }
}

/// Flag-summary code (`flags.summary`): 0 green, 1 local yellow, 2 full-course yellow / safety car, 3 blue,
/// 4 red, 5 chequered; `None` when no flag information exists.
pub const fn flag_summary_code(flag: RaceFlag) -> Option<u8> {
    match flag {
        RaceFlag::Green => Some(0),
        RaceFlag::Yellow => Some(1),
        RaceFlag::FullCourseYellow => Some(2),
        RaceFlag::Blue => Some(3),
        RaceFlag::Red => Some(4),
        RaceFlag::Chequered => Some(5),
        RaceFlag::None => None,
    }
}

fn band(formula: &'static str, b: Band) -> Out {
    Out {
        formula,
        provenance: Provenance::Derived,
        value: band_code(b).map(|c| Num::Int(i64::from(c))),
    }
}

/// The part of `gap` that belongs to a channel of formula `want`: `None` when the gap was computed by the
/// other classification's formula.
fn gap_part(gap: &Gap, want: &'static str, pick: impl Fn(&Gap) -> Option<Num>) -> Out {
    Out {
        formula: want,
        provenance: gap.provenance,
        value: (gap.formula == want).then(|| pick(gap)).flatten(),
    }
}

fn gap_seconds(gap: &Gap) -> Option<Num> {
    gap.seconds.map(Num::Float)
}

fn gap_laps(gap: &Gap) -> Option<Num> {
    gap.laps.map(|l| Num::Int(i64::from(l)))
}

/// Reads one output of an engine snapshot (a non-capturing closure).
type Pick<S, R> = fn(&S) -> R;

/// Per-wheel tyre outputs in [`EXTENDED_CHANNELS`] order (one group per entry).
const TYRE_WHEEL_GROUPS: [(ChannelId, Pick<WheelTyreState, Out>); 11] = [
    (ch::TYRE_PRESSURE_DELTA_FL, |w| {
        Out::float(w.pressure_delta_pa)
    }),
    (ch::TYRE_LAYER_TEMP_AVG_FL, |w| Out::float(w.layer_avg_c)),
    (ch::TYRE_TEMP_DELTA_FL, |w| {
        Out::float(w.surface_delta_vs_start_c)
    }),
    (ch::TYRE_CARCASS_TEMP_DELTA_FL, |w| {
        Out::float(w.carcass_delta_vs_start_c)
    }),
    (ch::TYRE_TEMP_PEAK_FL, |w| Out::float(w.peak_surface_c)),
    (ch::TYRE_TEMP_ROLLING_AVG_FL, |w| {
        Out::float(w.rolling_avg_surface_c)
    }),
    (ch::TYRE_TEMP_INNER_MIDDLE_FL, |w| {
        Out::float(w.inner_middle_delta_c)
    }),
    (ch::TYRE_TEMP_MIDDLE_OUTER_FL, |w| {
        Out::float(w.middle_outer_delta_c)
    }),
    (ch::TYRE_TEMP_BAND_FL, |w| {
        band(tyre::formula::BAND, w.surface_band)
    }),
    (ch::TYRE_PRESSURE_BAND_FL, |w| {
        band(tyre::formula::BAND, w.pressure_band)
    }),
    (ch::TYRE_EDGE_SPREAD_BAND_FL, |w| {
        band(tyre::formula::BAND, w.edge_spread_band)
    }),
];

/// Per-wheel brake outputs in [`EXTENDED_CHANNELS`] order.
const BRAKE_WHEEL_GROUPS: [(ChannelId, Pick<WheelBrakeState, Out>); 2] = [
    (ch::BRAKE_TEMP_PEAK_FL, |w| Out::float(w.peak_c)),
    (ch::BRAKE_TEMP_BAND_FL, |w| {
        band(brake::formula::BAND, w.band)
    }),
];

/// Writes the extended engine outputs into `frame` (see the module docs).
pub fn publish_extended(frame: &mut TelemetryFrame, out: &ExtendedOutputs<'_>) -> PublishReport {
    let mut r = PublishReport::default();

    for (fl, pick) in TYRE_WHEEL_GROUPS {
        write_wheels(frame, fl, &mut r, |wheel| {
            out.tyre
                .map_or(Out::unavailable(), |t| pick(&t.wheels[wheel.index()]))
        });
    }
    let tyre_axles: [(ChannelId, Pick<TyreSnapshot, Metric<f64>>); 4] = [
        (ch::TYRE_TEMP_BALANCE_FRONT_REAR, |t| {
            t.front_rear_temp_balance_c
        }),
        (ch::TYRE_TEMP_BALANCE_LEFT_RIGHT, |t| {
            t.left_right_temp_balance_c
        }),
        (ch::TYRE_PRESSURE_BALANCE_FRONT_REAR, |t| {
            t.front_rear_pressure_balance_pa
        }),
        (ch::TYRE_PRESSURE_BALANCE_LEFT_RIGHT, |t| {
            t.left_right_pressure_balance_pa
        }),
    ];
    for (id, pick) in tyre_axles {
        let o = out.tyre.map_or(Out::unavailable(), |t| Out::float(pick(t)));
        write(frame, id, o, &mut r);
    }

    for (fl, pick) in BRAKE_WHEEL_GROUPS {
        write_wheels(frame, fl, &mut r, |wheel| {
            out.brake
                .map_or(Out::unavailable(), |b| pick(&b.wheels[wheel.index()]))
        });
    }
    let (delta, bias_front) = match out.brake {
        Some(b) => (Out::float(b.front_rear_delta_c), Out::float(b.bias_front)),
        None => (Out::unavailable(), Out::unavailable()),
    };
    write(frame, ch::BRAKE_TEMP_FRONT_REAR, delta, &mut r);
    write(frame, ch::BRAKE_BIAS_FRONT, bias_front, &mut r);

    let map_dist = out.map_lap_distance.map_or(Out::unavailable(), Out::float);
    write(frame, ch::MAP_LAP_DISTANCE, map_dist, &mut r);

    let race_outs: [(ChannelId, Pick<RaceSnapshot, Out>); 10] = [
        (ch::CLASS_GAP_AHEAD, |s| {
            gap_part(
                &s.class_gap_ahead,
                formula::GAP_FROM_LEADER_TIMES,
                gap_seconds,
            )
        }),
        (ch::CLASS_GAP_BEHIND, |s| {
            gap_part(
                &s.class_gap_behind,
                formula::GAP_FROM_LEADER_TIMES,
                gap_seconds,
            )
        }),
        (ch::CLASS_GAP_AHEAD_LAPS, |s| {
            gap_part(&s.class_gap_ahead, formula::GAP_FROM_LEADER_TIMES, gap_laps)
        }),
        (ch::CLASS_GAP_BEHIND_LAPS, |s| {
            gap_part(
                &s.class_gap_behind,
                formula::GAP_FROM_LEADER_TIMES,
                gap_laps,
            )
        }),
        (ch::BEST_LAP_GAP_AHEAD, |s| {
            gap_part(&s.gap_ahead, formula::GAP_BEST_LAP, gap_seconds)
        }),
        (ch::BEST_LAP_GAP_BEHIND, |s| {
            gap_part(&s.gap_behind, formula::GAP_BEST_LAP, gap_seconds)
        }),
        (ch::CLASS_BEST_LAP_GAP_AHEAD, |s| {
            gap_part(&s.class_gap_ahead, formula::GAP_BEST_LAP, gap_seconds)
        }),
        (ch::CLASS_BEST_LAP_GAP_BEHIND, |s| {
            gap_part(&s.class_gap_behind, formula::GAP_BEST_LAP, gap_seconds)
        }),
        (ch::FLAG_SUMMARY, |s| Out {
            formula: formula::RACE_FLAG_SUMMARY,
            provenance: Provenance::Derived,
            value: flag_summary_code(s.flags.summary()).map(|c| Num::Int(i64::from(c))),
        }),
        (ch::FORMATION_LAP, |s| Out {
            formula: formula::FORMATION,
            provenance: Provenance::Derived,
            value: s.flags.formation.map(Num::Bool),
        }),
    ];
    for (id, pick) in race_outs {
        let o = out.race.map_or(Out::unavailable(), pick);
        write(frame, id, o, &mut r);
    }

    let (tc, abs) = match out.aids {
        Some(a) => (Out::float(a.tc_duty), Out::float(a.abs_duty)),
        None => (Out::unavailable(), Out::unavailable()),
    };
    write(frame, ch::TC_DUTY, tc, &mut r);
    write(frame, ch::ABS_DUTY, abs, &mut r);

    debug!(
        component = COMPONENT,
        valid = r.valid,
        unavailable = r.unavailable,
        invalid = r.out_of_range.len(),
        errors = r.errors.len(),
        "extended engine outputs published"
    );
    r
}

/// Writes one per-wheel group given its front-left id.
fn write_wheels(
    frame: &mut TelemetryFrame,
    front_left: ChannelId,
    r: &mut PublishReport,
    pick: impl Fn(Wheel) -> Out,
) {
    let Some(ids) = registry::wheel_group(front_left) else {
        r.errors.push(PublishError::UnknownChannel(front_left.0));
        return;
    };
    debug_assert_eq!(ids.len(), WHEELS);
    for wheel in Wheel::ALL {
        write(frame, ids[wheel.index()], pick(wheel), r);
    }
}

fn write(frame: &mut TelemetryFrame, id: ChannelId, o: Out, r: &mut PublishReport) {
    let Some(def) = registry::by_id(id) else {
        r.errors.push(PublishError::UnknownChannel(id.0));
        return;
    };
    let sample = match o.value {
        None => {
            r.unavailable += 1;
            Sample::unavailable()
        }
        Some(v) => {
            let contract = Metric::new(o.provenance, o.formula, Some(()));
            let contract_ok = check_contract(def, &contract, r);
            let in_range = match (v.ranged(), def.valid_range) {
                (Some(x), Some((lo, hi))) => x.is_finite() && x >= lo && x <= hi,
                (Some(x), None) => x.is_finite(),
                (None, _) => true,
            };
            if !contract_ok {
                Sample::invalid()
            } else if !in_range {
                debug!(
                    component = COMPONENT,
                    channel = def.name,
                    value = ?v,
                    "calculated value outside the registry range: published INVALID"
                );
                r.out_of_range.push(id);
                Sample::invalid()
            } else if let Some(value) = v.to_value(def.value_type) {
                r.valid += 1;
                Sample::valid(value)
            } else {
                r.errors.push(PublishError::NotRepresentable {
                    channel: id.0,
                    value_type: def.value_type,
                });
                Sample::invalid()
            }
        }
    };
    if let Err(source) = frame.set(id, sample) {
        warn!(component = COMPONENT, channel = def.name, error = %source, "sample rejected");
        r.errors.push(PublishError::Frame {
            channel: id.0,
            source,
        });
    }
}

#[cfg(test)]
mod tests;

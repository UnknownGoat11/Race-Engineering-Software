//! Publication of engine outputs into the registry's calculated channels (D-010, ARCHITECTURE §4 "Derived
//! metrics (race-engines, tagged DERIVED/ESTIMATED with formula id)").
//!
//! Rules:
//! - every channel owned by this half of `race-engines` ([`OWNED_CHANNELS`]) is written on every call, so a
//!   value never lingers from an earlier frame;
//! - a metric without a value (missing or `Unavailable` input) is written `Unavailable`, never 0;
//! - a value outside the channel's registry range is written `Invalid` (never clamped);
//! - the metric's formula must be the channel's registry origin (`formula:<id>`) and its provenance the
//!   channel's provenance; a mismatch is a programming error: the value is written `Invalid` (the drift is
//!   visible on the wire, never presented as a valid value under the wrong contract) and reported in
//!   [`PublishReport::errors`], logged at debug level only (this runs at the frame rate);
//! - `app.calculation_engine_version` carries [`CALCULATION_ENGINE_VERSION`].
//!
//! Engines whose outputs are not passed (`None`) get all their channels written `Unavailable`. Energy (VE),
//! tyre, brake and alert channels belong to the other engines and are not touched here.

use telemetry_model::frame::FrameError;
use telemetry_model::registry;
use telemetry_model::{
    ChannelDef, ChannelId, Provenance, Sample, TelemetryFrame, Value, ValueType, ch,
};
use thiserror::Error;
use tracing::{debug, warn};

use crate::CALCULATION_ENGINE_VERSION;
use crate::common::Metric;
use crate::fuel::FuelEstimate;
use crate::race::RaceSnapshot;
use crate::stint::StintSnapshot;
use crate::timing::TimingSnapshot;

/// Tracing component name.
const COMPONENT: &str = "race_engines::publish";

/// Prefix of a formula origin in the registry.
const FORMULA_ORIGIN_PREFIX: &str = "formula:";

/// Channels written by [`publish`].
pub const OWNED_CHANNELS: &[ChannelId] = &[
    ch::DELTA_PERSONAL_BEST,
    ch::PREDICTED_LAP_TIME,
    ch::FUEL_USED_LAST_LAP,
    ch::FUEL_PER_LAP,
    ch::FUEL_USED_THIS_LAP,
    ch::FUEL_LAPS_REMAINING,
    ch::FUEL_TIME_REMAINING,
    ch::FUEL_TO_FINISH,
    ch::FUEL_TO_ADD,
    ch::FUEL_MARGIN_LAPS,
    ch::STINT_NUMBER,
    ch::STINT_LAPS,
    ch::STINT_TIME,
    ch::STINT_MAX_LAPS,
    ch::TYRE_SET_LAPS,
    ch::RACE_LAPS_REMAINING,
    ch::CALC_ENGINE_VERSION,
];

/// Engine outputs of one publication step.
#[derive(Debug, Clone, Copy, Default)]
pub struct EngineOutputs<'a> {
    /// Timing engine snapshot.
    pub timing: Option<&'a TimingSnapshot>,
    /// Fuel engine estimate.
    pub fuel: Option<&'a FuelEstimate>,
    /// Stint engine snapshot.
    pub stint: Option<&'a StintSnapshot>,
    /// Stint length limit ([`crate::stint::stint_max_laps`]).
    pub stint_max_laps: Option<Metric<f64>>,
    /// Race engine snapshot.
    pub race: Option<&'a RaceSnapshot>,
}

/// A channel that could not be written as intended.
#[derive(Debug, Error, PartialEq)]
pub enum PublishError {
    /// The frame rejected the sample.
    #[error("channel 0x{channel:04X}: {source}")]
    Frame {
        /// Channel id.
        channel: u16,
        /// Cause.
        #[source]
        source: FrameError,
    },
    /// The value cannot be represented in the channel's value type.
    #[error("channel 0x{channel:04X}: value not representable as {value_type:?}")]
    NotRepresentable {
        /// Channel id.
        channel: u16,
        /// Registry type.
        value_type: ValueType,
    },
    /// The metric's formula or provenance differs from the registry definition.
    #[error(
        "channel 0x{channel:04X}: metric {formula} / {provenance:?} does not match the registry origin {origin}"
    )]
    ContractMismatch {
        /// Channel id.
        channel: u16,
        /// Metric formula.
        formula: &'static str,
        /// Metric provenance.
        provenance: Provenance,
        /// Registry origin.
        origin: &'static str,
    },
    /// The channel is not in the registry.
    #[error("channel 0x{0:04X} is not in the registry")]
    UnknownChannel(u16),
}

/// What one publication step wrote.
#[derive(Debug, Default, PartialEq)]
pub struct PublishReport {
    /// Channels written `Valid`.
    pub valid: usize,
    /// Channels written `Unavailable`.
    pub unavailable: usize,
    /// Channels written `Invalid` (value outside the registry range).
    pub out_of_range: Vec<ChannelId>,
    /// Problems (at most one per channel).
    pub errors: Vec<PublishError>,
}

/// A number an engine metric can carry.
trait ChannelNumber: Copy {
    fn to_value(self, value_type: ValueType) -> Option<Value>;
    fn to_f64(self) -> f64;
}

impl ChannelNumber for f64 {
    fn to_value(self, value_type: ValueType) -> Option<Value> {
        if !self.is_finite() {
            return None;
        }
        match value_type {
            // The registry checks that its f32 bounds are representable; a range-checked value fits.
            ValueType::F32 => Some(Value::F32(self as f32)),
            ValueType::F64 => Some(Value::F64(self)),
            // Never round a calculated float into an integer channel.
            _ => None,
        }
    }

    fn to_f64(self) -> f64 {
        self
    }
}

impl ChannelNumber for u32 {
    fn to_value(self, value_type: ValueType) -> Option<Value> {
        match value_type {
            ValueType::U32 => Some(Value::U32(self)),
            ValueType::U64 => Some(Value::U64(u64::from(self))),
            ValueType::I32 => i32::try_from(self).ok().map(Value::I32),
            ValueType::U8 => u8::try_from(self).ok().map(Value::U8),
            ValueType::F32 => Some(Value::F32(self as f32)),
            ValueType::F64 => Some(Value::F64(f64::from(self))),
            _ => None,
        }
    }

    fn to_f64(self) -> f64 {
        f64::from(self)
    }
}

/// Writes the engine outputs into `frame` (see the module docs).
pub fn publish(frame: &mut TelemetryFrame, out: &EngineOutputs<'_>) -> PublishReport {
    let mut r = PublishReport::default();
    let f64_none = |formula: &'static str| Metric::<f64>::derived(formula, None);
    let u32_none = |formula: &'static str| Metric::<u32>::derived(formula, None);

    let (delta_pb, predicted) = match out.timing {
        Some(t) => (t.delta_to_personal_best, t.predicted_lap),
        None => (f64_none(""), f64_none("")),
    };
    write(frame, ch::DELTA_PERSONAL_BEST, delta_pb, &mut r);
    write(frame, ch::PREDICTED_LAP_TIME, predicted, &mut r);

    let fuel_channels = [
        ch::FUEL_USED_LAST_LAP,
        ch::FUEL_PER_LAP,
        ch::FUEL_USED_THIS_LAP,
        ch::FUEL_LAPS_REMAINING,
        ch::FUEL_TIME_REMAINING,
        ch::FUEL_TO_FINISH,
        ch::FUEL_TO_ADD,
        ch::FUEL_MARGIN_LAPS,
    ];
    let fuel_metrics = match out.fuel {
        Some(f) => [
            f.fuel_used_last_lap,
            f.fuel_per_lap,
            f.fuel_used_this_lap,
            f.laps_remaining,
            f.time_remaining,
            f.fuel_to_finish,
            f.fuel_to_add,
            f.margin_laps,
        ],
        None => [f64_none(""); 8],
    };
    for (id, m) in fuel_channels.into_iter().zip(fuel_metrics) {
        write(frame, id, m, &mut r);
    }

    let (number, laps, time, tyre_set) = match out.stint {
        Some(s) => (s.number, s.laps, s.time, s.tyre_set_laps),
        None => (u32_none(""), u32_none(""), f64_none(""), u32_none("")),
    };
    write(frame, ch::STINT_NUMBER, number, &mut r);
    write(frame, ch::STINT_LAPS, laps, &mut r);
    write(frame, ch::STINT_TIME, time, &mut r);
    write(frame, ch::TYRE_SET_LAPS, tyre_set, &mut r);
    write(
        frame,
        ch::STINT_MAX_LAPS,
        out.stint_max_laps.unwrap_or(f64_none("")),
        &mut r,
    );

    let race_laps = out.race.map_or(f64_none(""), |s| s.race_laps_remaining);
    write(frame, ch::RACE_LAPS_REMAINING, race_laps, &mut r);

    // The engine version is a property of this build, not a calculation: always valid.
    match frame.set_valid(
        ch::CALC_ENGINE_VERSION,
        Value::U32(CALCULATION_ENGINE_VERSION),
    ) {
        Ok(()) => r.valid += 1,
        Err(source) => r.errors.push(PublishError::Frame {
            channel: ch::CALC_ENGINE_VERSION.0,
            source,
        }),
    }
    debug!(
        component = COMPONENT,
        valid = r.valid,
        unavailable = r.unavailable,
        invalid = r.out_of_range.len(),
        errors = r.errors.len(),
        "engine outputs published"
    );
    r
}

fn write<T: ChannelNumber>(
    frame: &mut TelemetryFrame,
    id: ChannelId,
    metric: Metric<T>,
    r: &mut PublishReport,
) {
    let Some(def) = registry::by_id(id) else {
        r.errors.push(PublishError::UnknownChannel(id.0));
        return;
    };
    let sample = match metric.value {
        None => {
            r.unavailable += 1;
            Sample::unavailable()
        }
        Some(v) => {
            let contract_ok = check_contract(def, &metric, r);
            let x = v.to_f64();
            let in_range = def
                .valid_range
                .is_none_or(|(lo, hi)| x.is_finite() && x >= lo && x <= hi);
            if !contract_ok {
                Sample::invalid()
            } else if !in_range {
                debug!(
                    component = COMPONENT,
                    channel = def.name,
                    value = x,
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

/// Checks a metric's formula and provenance against the registry contract of its channel. A mismatch is a
/// build defect (a renamed formula id): the value is published `Invalid` so the drift is visible on the wire,
/// and the error goes to [`PublishReport::errors`] for the caller to surface. It is logged at debug level
/// only, because this runs at the frame rate and a warning per frame would flood the log.
pub(crate) fn check_contract<T>(
    def: &ChannelDef,
    metric: &Metric<T>,
    r: &mut PublishReport,
) -> bool {
    let origin_ok = def
        .origin
        .strip_prefix(FORMULA_ORIGIN_PREFIX)
        .is_some_and(|f| f == metric.formula);
    let ok = origin_ok && def.provenance == metric.provenance;
    if !ok {
        debug!(
            component = COMPONENT,
            channel = def.name,
            formula = metric.formula,
            origin = def.origin,
            "metric does not match the registry contract"
        );
        r.errors.push(PublishError::ContractMismatch {
            channel: def.id.0,
            formula: metric.formula,
            provenance: metric.provenance,
            origin: def.origin,
        });
    }
    ok
}

#[cfg(test)]
mod tests;

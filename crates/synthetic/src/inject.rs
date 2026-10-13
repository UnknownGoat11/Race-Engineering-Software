//! Malformed-value injection (scenario "malformed") and a wire helper that reproduces what a broken source
//! would put on the wire.
//!
//! A [`TelemetryFrame`] can never hold a `Valid` NaN or infinity (`TelemetryFrame::set` refuses them), so
//! a NaN/∞ injection is recorded in the frame as `Invalid` (what the normalizer publishes for such a read)
//! and reported as an [`Injection`]. [`encode_snapshot_with_injections`] writes the raw non-finite value
//! as a `Valid` TLV record, to test decoders' own defences. Out-of-range values and spikes are finite and
//! are written into the frame as `Valid`: downstream validation must catch them.

use rand::Rng;
use rand_chacha::ChaCha8Rng;
use telemetry_model::TELEMETRY_SCHEMA_VERSION;
use telemetry_model::channel::ChannelId;
use telemetry_model::codec::{TlvEncoder, TlvHeader};
use telemetry_model::frame::{FrameError, TelemetryFrame};
use telemetry_model::registry;
use telemetry_model::value::{Sample, Value, ValueType};

use crate::error::SyntheticError;
use crate::scenario::{MalformedKind, MalformedSpec};

/// An out-of-range value lies this many range widths beyond the maximum.
const OUT_OF_RANGE_WIDTHS: f64 = 10.0;
/// A spike moves the value by this share of the plausible range.
const SPIKE_RANGE_SHARE: f64 = 0.45;

/// One injected malformed value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Injection {
    pub channel: ChannelId,
    pub kind: MalformedKind,
    /// The value the "source" produced: NaN / ±∞ for those kinds, otherwise the value written as `Valid`.
    pub raw_value: f64,
}

impl Injection {
    /// True for NaN/∞ injections (stored as `Invalid` in the frame).
    pub fn is_non_finite(&self) -> bool {
        matches!(self.kind, MalformedKind::NaN | MalformedKind::Infinity)
    }
}

/// With probability `spec.probability` corrupts one channel of `frame`.
pub(crate) fn inject(
    frame: &mut TelemetryFrame,
    spec: &MalformedSpec,
    et_s: f64,
    rng: &mut ChaCha8Rng,
) -> Result<Option<Injection>, SyntheticError> {
    if !(spec.start_et_s..=spec.end_et_s).contains(&et_s) || !rng.random_bool(spec.probability) {
        return Ok(None);
    }
    let (Some(&channel), Some(&kind)) = (
        spec.channels.get(rng.random_range(0..spec.channels.len())),
        spec.kinds.get(rng.random_range(0..spec.kinds.len())),
    ) else {
        return Ok(None);
    };
    let def = registry::by_id(channel).ok_or(FrameError::UnknownChannel(channel.0))?;
    let Some((min, max)) = def.valid_range else {
        return Ok(None);
    };
    let raw_value = match kind {
        MalformedKind::NaN => f64::NAN,
        MalformedKind::Infinity => {
            if rng.random_bool(0.5) {
                f64::INFINITY
            } else {
                f64::NEG_INFINITY
            }
        }
        MalformedKind::OutOfRange => max + OUT_OF_RANGE_WIDTHS * (max - min).abs().max(1.0),
        MalformedKind::Spike => {
            let current = frame.f64(channel).unwrap_or((min + max) / 2.0);
            let jump = SPIKE_RANGE_SHARE * (max - min);
            let spiked = if current < (min + max) / 2.0 {
                current + jump
            } else {
                current - jump
            };
            spiked.clamp(min, max)
        }
    };
    if raw_value.is_finite() {
        frame.set_valid(channel, typed(def.value_type, raw_value))?;
    } else {
        frame.set(channel, Sample::invalid())?;
    }
    Ok(Some(Injection {
        channel,
        kind,
        raw_value,
    }))
}

/// `value` in the channel's float wire type.
fn typed(value_type: ValueType, value: f64) -> Value {
    match value_type {
        // Narrowing is the point: this is the wire representation of the injected value.
        ValueType::F32 => Value::F32(value as f32),
        _ => Value::F64(value),
    }
}

/// Appends a snapshot TLV block of `frame` (like [`TelemetryFrame::encode_snapshot`]) in which every
/// NaN/∞ injection is written as the raw non-finite value with quality `Valid`, as a broken producer would.
/// Returns the record count.
pub fn encode_snapshot_with_injections(
    frame: &TelemetryFrame,
    injections: &[Injection],
    buf: &mut Vec<u8>,
    capture_relay_ns_est: i64,
) -> Result<usize, SyntheticError> {
    let header = TlvHeader {
        schema_version: TELEMETRY_SCHEMA_VERSION,
        capture_mono_ns: frame.capture_mono_ns,
        capture_relay_ns_est,
        source_et_s: frame.source_et_s.unwrap_or(f64::NAN),
    };
    let mut enc = TlvEncoder::begin(buf, &header);
    for (def, sample) in frame.iter() {
        let raw = injections
            .iter()
            .find(|i| i.channel == def.id && i.is_non_finite());
        match raw {
            Some(i) => enc.push(def.id, &Sample::valid(typed(def.value_type, i.raw_value)))?,
            None => enc.push(def.id, sample)?,
        }
    }
    Ok(enc.finish())
}

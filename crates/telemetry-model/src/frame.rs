//! The canonical telemetry frame: one dense sample per registry channel.
//!
//! A frame is built (and validated) by its producer, then published as an [`SharedFrame`]
//! (`Arc<TelemetryFrame>`); consumers only ever get shared references, so a published frame is immutable.
//!
//! TLV conversion (docs/PROTOCOL.md §4): a *snapshot* encodes every channel; a *delta* encodes only the
//! channels whose sample changed relative to a previous frame. The default float change epsilon is 0, so a
//! delta never loses data. Decoding skips channel ids this build does not know (forward compatibility).

use std::sync::Arc;

use thiserror::Error;

use crate::TELEMETRY_SCHEMA_VERSION;
use crate::channel::{ChannelDef, ChannelId};
use crate::codec::{self, CodecError, TlvEncoder, TlvHeader};
use crate::registry;
use crate::value::{MAX_STR_VALUE_BYTES, Quality, Sample, Value, ValueType};

/// A published, immutable frame.
pub type SharedFrame = Arc<TelemetryFrame>;

/// Returned by [`TelemetryFrame::get`] for ids this build does not know.
static UNKNOWN_CHANNEL_SAMPLE: Sample = Sample {
    value: Value::None,
    quality: Quality::NotSupported,
};

/// Errors raised when writing a sample into a frame or decoding a frame.
#[derive(Debug, Error, PartialEq)]
pub enum FrameError {
    #[error("channel 0x{0:04X} is not in the registry")]
    UnknownChannel(u16),
    #[error(
        "channel {channel}: value type {actual:?} does not match the registry type {expected:?}"
    )]
    TypeMismatch {
        channel: &'static str,
        expected: ValueType,
        actual: ValueType,
    },
    #[error("channel {0}: a VALID sample must carry a value")]
    ValidWithoutValue(&'static str),
    #[error("channel {0}: a VALID float sample must be finite")]
    NonFinite(&'static str),
    #[error("channel {channel}: string value is {len} bytes, maximum is {MAX_STR_VALUE_BYTES}")]
    StringTooLong { channel: &'static str, len: usize },
    #[error("TLV codec error: {0}")]
    Codec(#[from] CodecError),
}

/// What happened while applying a TLV block to a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeReport {
    /// Schema version announced by the block header.
    pub schema_version: u16,
    /// Records applied to the frame.
    pub applied: usize,
    /// Records skipped because the channel id is unknown to this build.
    pub unknown_skipped: usize,
    /// Records whose value contradicted the registry (wrong type, non-finite float, valid without value);
    /// they are stored as `Invalid`, never dropped silently.
    pub invalidated: usize,
}

/// One telemetry frame: header plus a dense sample per registry channel (indexed by registry position).
#[derive(Debug, Clone, PartialEq)]
pub struct TelemetryFrame {
    /// Producer sequence number (monotonic per stream).
    pub sequence: u64,
    /// Driver monotonic nanoseconds at the moment the LMU data was read.
    pub capture_mono_ns: u64,
    /// LMU elapsed session time of the sample (`mElapsedTime`), `None` when unknown.
    pub source_et_s: Option<f64>,
    values: Vec<Sample>,
}

impl TelemetryFrame {
    /// A frame where every channel carries its "no data" sample (`Unavailable`, or `NotSupported` for
    /// channels no source provides). Never zeros.
    pub fn new(sequence: u64, capture_mono_ns: u64, source_et_s: Option<f64>) -> Self {
        Self {
            sequence,
            capture_mono_ns,
            source_et_s: finite(source_et_s),
            values: registry::default_samples(),
        }
    }

    /// Wraps the frame for publication; from here on it is immutable.
    pub fn into_shared(self) -> SharedFrame {
        Arc::new(self)
    }

    /// Sample of `id`; ids unknown to this build read as `NotSupported`.
    pub fn get(&self, id: ChannelId) -> &Sample {
        registry::index_of(id)
            .and_then(|i| self.values.get(i))
            .unwrap_or(&UNKNOWN_CHANNEL_SAMPLE)
    }

    /// Numeric value of `id`, only if the sample is `Valid` (strings and missing values yield `None`).
    pub fn f64(&self, id: ChannelId) -> Option<f64> {
        let s = self.get(id);
        if s.is_valid() { s.value.as_f64() } else { None }
    }

    /// Sample at registry position `index`.
    pub fn sample_at(&self, index: usize) -> Option<&Sample> {
        self.values.get(index)
    }

    /// Writes a sample after checking it against the registry: the value type must match the channel's
    /// type, a `Valid` sample must carry a finite value, strings must fit the wire limit. Non-valid samples
    /// are stored without a value, so an invalid or stale value can never be read back as a measurement.
    pub fn set(&mut self, id: ChannelId, sample: Sample) -> Result<(), FrameError> {
        let index = registry::index_of(id).ok_or(FrameError::UnknownChannel(id.0))?;
        let def = &registry::CHANNELS[index];
        let checked = check_sample(def, sample)?;
        if let Some(slot) = self.values.get_mut(index) {
            *slot = checked;
        }
        Ok(())
    }

    /// Shorthand for `set(id, Sample::valid(value))`.
    pub fn set_valid(&mut self, id: ChannelId, value: Value) -> Result<(), FrameError> {
        self.set(id, Sample::valid(value))
    }

    /// Downgrades the sample at `index` to `quality` (dropping its value). Used by validation; a `Valid`
    /// quality is ignored because a value cannot be made valid this way.
    pub(crate) fn downgrade_at(&mut self, index: usize, quality: Quality) {
        if quality == Quality::Valid {
            return;
        }
        if let Some(slot) = self.values.get_mut(index) {
            *slot = Sample {
                value: Value::None,
                quality,
            };
        }
    }

    /// Every channel with its sample, in registry order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static ChannelDef, &Sample)> {
        registry::CHANNELS.iter().zip(&self.values)
    }

    /// Channels whose sample is `Valid`.
    pub fn iter_valid(&self) -> impl Iterator<Item = (&'static ChannelDef, &Sample)> {
        self.iter().filter(|(_, s)| s.is_valid())
    }

    /// Number of `Valid` samples.
    pub fn valid_count(&self) -> usize {
        self.values.iter().filter(|s| s.is_valid()).count()
    }

    /// Number of channels (= registry length).
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Always false for a registry with channels; provided for API completeness.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    fn header(&self, capture_relay_ns_est: i64) -> TlvHeader {
        TlvHeader {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            capture_mono_ns: self.capture_mono_ns,
            capture_relay_ns_est,
            source_et_s: self.source_et_s.unwrap_or(f64::NAN),
        }
    }

    /// Appends a snapshot TLV block (every channel) to `buf`. Returns the record count.
    pub fn encode_snapshot(
        &self,
        buf: &mut Vec<u8>,
        capture_relay_ns_est: i64,
    ) -> Result<usize, FrameError> {
        let mut enc = TlvEncoder::begin(buf, &self.header(capture_relay_ns_est));
        for (def, sample) in self.iter() {
            enc.push(def.id, sample)?;
        }
        Ok(enc.finish())
    }

    /// Appends a delta TLV block (only channels that changed relative to `previous`) to `buf`, with a float
    /// epsilon of 0 for every channel (no data loss). Returns the record count.
    pub fn encode_delta(
        &self,
        previous: &TelemetryFrame,
        buf: &mut Vec<u8>,
        capture_relay_ns_est: i64,
    ) -> Result<usize, FrameError> {
        self.encode_delta_with_epsilon(previous, buf, capture_relay_ns_est, &|_| 0.0)
    }

    /// Like [`Self::encode_delta`] with a per-channel float epsilon: a valid float that moved by at most
    /// `epsilon(def)` is not sent. With a non-zero epsilon `previous` must be the state the receiver holds
    /// (the last *sent* values), otherwise small changes accumulate unseen.
    pub fn encode_delta_with_epsilon(
        &self,
        previous: &TelemetryFrame,
        buf: &mut Vec<u8>,
        capture_relay_ns_est: i64,
        epsilon: &dyn Fn(&ChannelDef) -> f64,
    ) -> Result<usize, FrameError> {
        let mut enc = TlvEncoder::begin(buf, &self.header(capture_relay_ns_est));
        for ((def, now), before) in self.iter().zip(&previous.values) {
            if changed(before, now, epsilon(def)) {
                enc.push(def.id, now)?;
            }
        }
        Ok(enc.finish())
    }

    /// Ids of channels whose sample differs from `previous` (epsilon 0).
    pub fn changed_channels<'a>(
        &'a self,
        previous: &'a TelemetryFrame,
    ) -> impl Iterator<Item = ChannelId> + 'a {
        self.iter()
            .zip(&previous.values)
            .filter(|((_, now), before)| changed(before, now, 0.0))
            .map(|((d, _), _)| d.id)
    }

    /// Decodes a snapshot block into a new frame (channels absent from the block keep their "no data"
    /// sample).
    pub fn from_snapshot(sequence: u64, data: &[u8]) -> Result<(Self, DecodeReport), FrameError> {
        let base = Self::new(sequence, 0, None);
        base.apply_tlv(sequence, data)
    }

    /// Applies a TLV block (snapshot or delta) on top of this frame and returns the resulting new frame; the
    /// header (capture time, source ET) comes from the block. Unknown ids are skipped; records that
    /// contradict the registry are stored as `Invalid` and counted.
    pub fn apply_tlv(
        &self,
        sequence: u64,
        data: &[u8],
    ) -> Result<(Self, DecodeReport), FrameError> {
        let (header, records) = codec::decode(data)?;
        let mut frame = self.clone();
        frame.sequence = sequence;
        frame.capture_mono_ns = header.capture_mono_ns;
        frame.source_et_s = finite(Some(header.source_et_s));
        let mut report = DecodeReport {
            schema_version: header.schema_version,
            ..DecodeReport::default()
        };
        for record in records {
            let Some(index) = registry::index_of(record.channel) else {
                report.unknown_skipped += 1;
                continue;
            };
            let def = &registry::CHANNELS[index];
            let sample = match check_sample(def, record.sample) {
                Ok(sample) => sample,
                Err(_) => {
                    report.invalidated += 1;
                    Sample::invalid()
                }
            };
            if let Some(slot) = frame.values.get_mut(index) {
                *slot = sample;
            }
            report.applied += 1;
        }
        Ok((frame, report))
    }
}

/// `Some(v)` only for finite `v`.
fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

/// Checks a sample against its channel definition and normalises non-valid samples to carry no value.
fn check_sample(def: &ChannelDef, sample: Sample) -> Result<Sample, FrameError> {
    if sample.quality != Quality::Valid {
        return Ok(Sample {
            value: Value::None,
            quality: sample.quality,
        });
    }
    let actual = sample.value.value_type();
    if actual == ValueType::None {
        return Err(FrameError::ValidWithoutValue(def.name));
    }
    if actual != def.value_type {
        return Err(FrameError::TypeMismatch {
            channel: def.name,
            expected: def.value_type,
            actual,
        });
    }
    match &sample.value {
        Value::F32(v) if !v.is_finite() => return Err(FrameError::NonFinite(def.name)),
        Value::F64(v) if !v.is_finite() => return Err(FrameError::NonFinite(def.name)),
        Value::Str(s) if s.len() > MAX_STR_VALUE_BYTES => {
            return Err(FrameError::StringTooLong {
                channel: def.name,
                len: s.len(),
            });
        }
        _ => {}
    }
    Ok(sample)
}

/// True if `now` must be transmitted given the receiver holds `before`.
fn changed(before: &Sample, now: &Sample, epsilon: f64) -> bool {
    if before.quality != now.quality {
        return true;
    }
    match (&before.value, &now.value) {
        (Value::F32(a), Value::F32(b)) => float_changed(f64::from(*a), f64::from(*b), epsilon),
        (Value::F64(a), Value::F64(b)) => float_changed(*a, *b, epsilon),
        (a, b) => a != b,
    }
}

fn float_changed(a: f64, b: f64, epsilon: f64) -> bool {
    // Epsilon 0 compares bit patterns: any representable change (including -0.0 → 0.0) is sent.
    if epsilon > 0.0 {
        (a - b).abs() > epsilon
    } else {
        a.to_bits() != b.to_bits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::RELAY_NS_UNKNOWN;
    use crate::registry::ch;

    fn sample_frame() -> TelemetryFrame {
        let mut f = TelemetryFrame::new(7, 1_000, Some(12.5));
        f.set_valid(ch::SPEED, Value::F32(78.0)).expect("speed");
        f.set_valid(ch::GEAR, Value::I32(4)).expect("gear");
        f.set_valid(ch::VEHICLE_NAME, Value::Str("Porsche 963".into()))
            .expect("name");
        f
    }

    #[test]
    fn new_frame_has_no_zeros() {
        let f = TelemetryFrame::new(0, 0, None);
        assert_eq!(f.len(), registry::len());
        assert_eq!(f.valid_count(), 0);
        assert_eq!(f.f64(ch::SPEED), None);
        assert_eq!(f.get(ch::SPEED).quality, Quality::Unavailable);
        assert_eq!(f.get(ChannelId(0xFFFF)).quality, Quality::NotSupported);
    }

    #[test]
    fn set_checks_types_and_finiteness() {
        let mut f = sample_frame();
        assert_eq!(f.f64(ch::SPEED), Some(78.0));
        assert!(matches!(
            f.set_valid(ch::SPEED, Value::F64(1.0)),
            Err(FrameError::TypeMismatch { .. })
        ));
        assert!(matches!(
            f.set_valid(ch::SPEED, Value::F32(f32::NAN)),
            Err(FrameError::NonFinite(_))
        ));
        assert!(matches!(
            f.set(ch::SPEED, Sample::valid(Value::None)),
            Err(FrameError::ValidWithoutValue(_))
        ));
        assert_eq!(
            f.set_valid(ChannelId(0xFFFE), Value::F32(1.0)),
            Err(FrameError::UnknownChannel(0xFFFE))
        );
        // A rejected write leaves the previous value untouched.
        assert_eq!(f.f64(ch::SPEED), Some(78.0));
        // Invalid samples never keep a value.
        f.set(
            ch::SPEED,
            Sample {
                value: Value::F32(5.0),
                quality: Quality::Invalid,
            },
        )
        .expect("invalid");
        assert_eq!(f.get(ch::SPEED), &Sample::invalid());
        assert_eq!(f.f64(ch::SPEED), None);
    }

    #[test]
    fn snapshot_roundtrip() {
        let f = sample_frame();
        let mut buf = Vec::new();
        let n = f
            .encode_snapshot(&mut buf, RELAY_NS_UNKNOWN)
            .expect("encode");
        assert_eq!(n, registry::len());
        let (g, report) = TelemetryFrame::from_snapshot(7, &buf).expect("decode");
        assert_eq!(report.applied, registry::len());
        assert_eq!(report.unknown_skipped, 0);
        assert_eq!(report.invalidated, 0);
        assert_eq!(report.schema_version, TELEMETRY_SCHEMA_VERSION);
        assert_eq!(g, f);
    }

    #[test]
    fn delta_carries_only_changes_and_reconstructs() {
        let prev = sample_frame();
        let mut next = prev.clone();
        next.sequence = 8;
        next.source_et_s = Some(12.51);
        next.set_valid(ch::SPEED, Value::F32(78.000_01))
            .expect("speed");
        next.set(ch::GEAR, Sample::stale()).expect("gear");
        let mut buf = Vec::new();
        let n = next
            .encode_delta(&prev, &mut buf, RELAY_NS_UNKNOWN)
            .expect("encode");
        assert_eq!(
            n, 2,
            "a tiny float change and a quality change must both be sent"
        );
        assert_eq!(
            next.changed_channels(&prev).collect::<Vec<_>>(),
            vec![ch::SPEED, ch::GEAR]
        );
        let (rebuilt, report) = prev.apply_tlv(8, &buf).expect("apply");
        assert_eq!(report.applied, 2);
        assert_eq!(rebuilt, next);

        let mut coarse = Vec::new();
        let n = next
            .encode_delta_with_epsilon(&prev, &mut coarse, RELAY_NS_UNKNOWN, &|_| 0.1)
            .expect("encode");
        assert_eq!(n, 1, "with an epsilon only the quality change remains");
    }

    #[test]
    fn decode_skips_unknown_and_invalidates_mismatches() {
        let mut buf = Vec::new();
        let header = TlvHeader {
            schema_version: 99,
            capture_mono_ns: 5,
            capture_relay_ns_est: 0,
            source_et_s: f64::NAN,
        };
        let mut enc = TlvEncoder::begin(&mut buf, &header);
        enc.push(ChannelId(0x0EFF), &Sample::valid(Value::F32(1.0)))
            .expect("unknown id");
        enc.push(ch::SPEED, &Sample::valid(Value::U8(3)))
            .expect("wrong type");
        enc.push(ch::RPM, &Sample::valid(Value::F32(7_000.0)))
            .expect("rpm");
        enc.finish();
        let (f, report) = TelemetryFrame::from_snapshot(1, &buf).expect("decode");
        assert_eq!(
            report,
            DecodeReport {
                schema_version: 99,
                applied: 2,
                unknown_skipped: 1,
                invalidated: 1
            }
        );
        assert_eq!(f.get(ch::SPEED), &Sample::invalid());
        assert_eq!(f.f64(ch::RPM), Some(7_000.0));
        assert_eq!(f.source_et_s, None);
        assert_eq!(f.capture_mono_ns, 5);
    }

    #[test]
    fn published_frames_are_shared() {
        let shared = sample_frame().into_shared();
        let other = Arc::clone(&shared);
        assert_eq!(other.f64(ch::GEAR), Some(4.0));
        assert_eq!(other.iter_valid().count(), 3);
    }
}

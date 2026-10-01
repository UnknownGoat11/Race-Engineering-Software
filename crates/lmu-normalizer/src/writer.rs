//! Typed, registry-checked writes into a [`TelemetryFrame`].
//!
//! Every write goes through the registry definition of the channel: floats are stored as the channel's
//! float type, integers are range-checked into the channel's integer type, and anything that cannot be
//! represented (NaN, ±Inf, an f64 overflowing f32, an integer out of the type's range) becomes
//! `Quality::Invalid` — never a clamped or zeroed value. A write the frame rejects (a type mismatch between
//! this crate and the registry, i.e. a bug) is recorded and logged, never panics.

use telemetry_model::frame::{FrameError, TelemetryFrame};
use telemetry_model::registry;
use telemetry_model::value::MAX_STR_VALUE_BYTES;
use telemetry_model::{ChannelId, Quality, Sample, Value, ValueType};

/// Writes samples into one frame.
pub struct FrameWriter<'f> {
    frame: &'f mut TelemetryFrame,
    errors: Vec<(ChannelId, FrameError)>,
    /// Test instrumentation: channels the mapping considered (any write, including "no data").
    #[cfg(test)]
    pub(crate) touched: std::collections::BTreeSet<ChannelId>,
    /// Test instrumentation: channels that received an actual value (not a value-less quality).
    #[cfg(test)]
    pub(crate) valued: std::collections::BTreeSet<ChannelId>,
}

impl<'f> FrameWriter<'f> {
    /// A writer over `frame`.
    pub fn new(frame: &'f mut TelemetryFrame) -> Self {
        Self {
            frame,
            errors: Vec::new(),
            #[cfg(test)]
            touched: std::collections::BTreeSet::new(),
            #[cfg(test)]
            valued: std::collections::BTreeSet::new(),
        }
    }

    /// Mapping errors (bugs) collected so far.
    pub fn into_errors(self) -> Vec<(ChannelId, FrameError)> {
        self.errors
    }

    fn put(&mut self, id: ChannelId, sample: Sample) {
        #[cfg(test)]
        {
            self.touched.insert(id);
            if sample.value != Value::None {
                self.valued.insert(id);
            }
        }
        if let Err(err) = self.frame.set(id, sample) {
            self.fail(id, err);
        }
    }

    fn fail(&mut self, id: ChannelId, err: FrameError) {
        tracing::error!(
            component = "lmu-normalizer",
            channel = id.0,
            error = %err,
            "frame rejected a normalized sample (mapping bug)"
        );
        self.errors.push((id, err));
    }

    /// Records that `id` is not of the kind this crate tried to write (a mapping bug).
    fn wrong_kind(&mut self, id: ChannelId, actual: ValueType) {
        let err = match registry::by_id(id) {
            Some(def) => FrameError::TypeMismatch {
                channel: def.name,
                expected: def.value_type,
                actual,
            },
            None => FrameError::UnknownChannel(id.0),
        };
        self.fail(id, err);
    }

    /// Marks `id` with a value-less quality (`Unavailable`, `NotSupported`, `Invalid`).
    pub fn quality(&mut self, id: ChannelId, quality: Quality) {
        self.put(
            id,
            Sample {
                value: Value::None,
                quality,
            },
        );
    }

    /// Leaves `id` at its "no data" default but records that it was considered (sentinel or missing input).
    pub fn none(&mut self, id: ChannelId) {
        #[cfg(test)]
        self.touched.insert(id);
        let _ = id;
    }

    /// A float in internal units. Non-finite (including f32 overflow) → `Invalid`.
    pub fn float(&mut self, id: ChannelId, v: f64) {
        let def = registry::by_id(id);
        let sample = match def.map(|d| d.value_type) {
            Some(ValueType::F64) if v.is_finite() => Sample::valid(Value::F64(v)),
            Some(ValueType::F32) => {
                // Registry storage type; f32 resolution is far finer than the physical resolution here.
                let narrowed = narrow_within(v, def.and_then(|d| d.valid_range));
                if narrowed.is_finite() {
                    Sample::valid(Value::F32(narrowed))
                } else {
                    Sample::invalid()
                }
            }
            Some(ValueType::F64) => Sample::invalid(),
            _ => return self.wrong_kind(id, ValueType::F64),
        };
        self.put(id, sample);
    }

    /// `Some` → [`FrameWriter::float`]; `None` → left at "no data".
    pub fn opt_float(&mut self, id: ChannelId, v: Option<f64>) {
        match v {
            Some(v) => self.float(id, v),
            None => self.none(id),
        }
    }

    /// An integer converted into the channel's integer type; out of range → `Invalid`.
    pub fn int(&mut self, id: ChannelId, v: i64) {
        let value_type = registry::by_id(id).map(|d| d.value_type);
        let value = match value_type {
            Some(ValueType::I32) => i32::try_from(v).ok().map(Value::I32),
            Some(ValueType::U32) => u32::try_from(v).ok().map(Value::U32),
            Some(ValueType::U8) => u8::try_from(v).ok().map(Value::U8),
            Some(ValueType::I8) => i8::try_from(v).ok().map(Value::I8),
            Some(ValueType::U64) => u64::try_from(v).ok().map(Value::U64),
            _ => return self.wrong_kind(id, ValueType::I32),
        };
        self.put(id, value.map_or_else(Sample::invalid, Sample::valid));
    }

    /// `Some` → [`FrameWriter::int`]; `None` → left at "no data".
    pub fn opt_int(&mut self, id: ChannelId, v: Option<i64>) {
        match v {
            Some(v) => self.int(id, v),
            None => self.none(id),
        }
    }

    /// A boolean.
    pub fn boolean(&mut self, id: ChannelId, v: bool) {
        self.put(id, Sample::valid(Value::Bool(v)));
    }

    /// `Some` → [`FrameWriter::text`]; `None` → left at "no data".
    pub fn opt_text(&mut self, id: ChannelId, s: Option<String>) {
        match s {
            Some(s) => self.text(id, &s),
            None => self.none(id),
        }
    }

    /// Text, truncated at a character boundary to the wire limit; empty text → left at "no data".
    pub fn text(&mut self, id: ChannelId, s: &str) {
        if s.is_empty() {
            self.none(id);
            return;
        }
        let mut end = s.len().min(MAX_STR_VALUE_BYTES);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.put(id, Sample::valid(Value::Str(s[..end].to_owned())));
    }
}

/// Narrows `v` to f32. Rounding to the nearest f32 can step just outside an inclusive range bound that
/// `v` itself satisfies (e.g. `atan2` returning exactly −π, whose nearest f32 is below −π); in that case
/// the adjacent f32 towards zero is used instead (a sub-ulp rounding choice, not a clamp: values outside the
/// range are narrowed normally and rejected by validation).
fn narrow_within(v: f64, range: Option<(f64, f64)>) -> f32 {
    let narrowed = v as f32;
    let Some((lo, hi)) = range else {
        return narrowed;
    };
    let wide = f64::from(narrowed);
    let v_inside = v >= lo && v <= hi;
    let rounded_out = wide < lo || wide > hi;
    if v_inside && rounded_out && narrowed.is_finite() && narrowed != 0.0 {
        // Decrementing the bit pattern of a finite non-zero float reduces its magnitude by one ulp.
        f32::from_bits(narrowed.to_bits() - 1)
    } else {
        narrowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use telemetry_model::ch;

    #[test]
    fn invalid_values_never_become_numbers() {
        let mut f = TelemetryFrame::new(1, 0, Some(1.0));
        let mut w = FrameWriter::new(&mut f);
        w.float(ch::RPM, f64::NAN);
        w.float(ch::SPEED, 1.0e300); // overflows f32
        w.float(ch::ELAPSED_TIME, f64::INFINITY);
        w.int(ch::POSITION, 300); // u8 overflow
        w.int(ch::GEAR, 3);
        w.text(ch::VEHICLE_NAME, "");
        assert!(w.into_errors().is_empty());
        for id in [ch::RPM, ch::SPEED, ch::ELAPSED_TIME, ch::POSITION] {
            assert_eq!(f.get(id).quality, Quality::Invalid, "{id:?}");
            assert_eq!(f.get(id).value, Value::None);
        }
        assert_eq!(f.get(ch::GEAR).value, Value::I32(3));
        assert_eq!(f.get(ch::VEHICLE_NAME).quality, Quality::Unavailable);
    }

    #[test]
    fn range_edges_survive_f32_narrowing() {
        let pi = std::f64::consts::PI;
        let range = Some((-pi, pi));
        assert!(f64::from(narrow_within(-pi, range)) >= -pi);
        assert!(f64::from(narrow_within(pi, range)) <= pi);
        // Outside the range: narrowed normally (validation rejects it).
        assert!(f64::from(narrow_within(4.0, range)) > pi);
        assert_eq!(narrow_within(0.5, None), 0.5);
    }

    #[test]
    fn long_text_is_truncated_at_a_char_boundary() {
        let mut f = TelemetryFrame::new(1, 0, None);
        let mut w = FrameWriter::new(&mut f);
        let long = "é".repeat(40); // 80 bytes
        w.text(ch::VEHICLE_NAME, &long);
        assert!(w.into_errors().is_empty());
        match &f.get(ch::VEHICLE_NAME).value {
            Value::Str(s) => assert!(s.len() <= MAX_STR_VALUE_BYTES && s.chars().all(|c| c == 'é')),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn type_mismatches_are_reported_not_panicking() {
        let mut f = TelemetryFrame::new(1, 0, None);
        let mut w = FrameWriter::new(&mut f);
        w.boolean(ch::RPM, true);
        assert_eq!(w.into_errors().len(), 1);
    }
}

//! Channel TLV codec (docs/PROTOCOL.md §4).
//!
//! Shared by the network protocol (TELEMETRY_DELTA / TELEMETRY_SNAPSHOT / WEATHER_UPDATE payloads) and the
//! local recorder, so live and recorded data use byte-identical encodings (spec §273).
//!
//! Layout (little-endian):
//! ```text
//! u16 schema_version | u64 capture_mono_ns | i64 capture_relay_ns_est | f64 source_et_s | u16 record_count
//! record_count × ( u16 channel_id | u8 tag | value bytes )
//! tag = (quality << 5) | value_type
//! ```

use thiserror::Error;

use crate::channel::ChannelId;
use crate::value::{MAX_STR_VALUE_BYTES, Quality, Sample, Value, ValueType};

/// Size of the fixed TLV block header in bytes.
pub const TLV_HEADER_BYTES: usize = 2 + 8 + 8 + 8 + 2;
/// Sentinel for "relay clock estimate unknown".
pub const RELAY_NS_UNKNOWN: i64 = i64::MIN;
/// Maximum number of records in one TLV block (bounded by the u16 count field).
pub const MAX_RECORDS: usize = u16::MAX as usize;

const QUALITY_SHIFT: u8 = 5;
const VALUE_TYPE_MASK: u8 = 0b0001_1111;

/// Fixed header preceding the records of a TLV block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TlvHeader {
    pub schema_version: u16,
    /// Driver monotonic nanoseconds at the moment the LMU data was read.
    pub capture_mono_ns: u64,
    /// Driver's estimate of the relay clock (ns since Unix epoch) at capture, or [`RELAY_NS_UNKNOWN`].
    pub capture_relay_ns_est: i64,
    /// LMU elapsed session time of the sample, NaN if unavailable.
    pub source_et_s: f64,
}

/// One decoded record.
#[derive(Debug, Clone, PartialEq)]
pub struct TlvRecord {
    pub channel: ChannelId,
    pub sample: Sample,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CodecError {
    #[error("TLV block truncated: needed {needed} more bytes at offset {offset}")]
    Truncated { offset: usize, needed: usize },
    #[error("unknown value type {0} in TLV record")]
    UnknownValueType(u8),
    #[error("unknown quality {0} in TLV record")]
    UnknownQuality(u8),
    #[error("string value is {0} bytes, maximum is {MAX_STR_VALUE_BYTES}")]
    StringTooLong(usize),
    #[error("string value is not valid UTF-8")]
    InvalidUtf8,
    #[error("too many records: {0}")]
    TooManyRecords(usize),
    #[error("{0} trailing bytes after the last TLV record")]
    TrailingBytes(usize),
}

/// Incremental TLV encoder writing into a caller-owned buffer (allocation-free on the hot path once the
/// buffer has grown to its steady-state size).
pub struct TlvEncoder<'a> {
    buf: &'a mut Vec<u8>,
    count_offset: usize,
    count: usize,
}

impl<'a> TlvEncoder<'a> {
    /// Starts a new block, appending the header to `buf`.
    pub fn begin(buf: &'a mut Vec<u8>, header: &TlvHeader) -> Self {
        buf.extend_from_slice(&header.schema_version.to_le_bytes());
        buf.extend_from_slice(&header.capture_mono_ns.to_le_bytes());
        buf.extend_from_slice(&header.capture_relay_ns_est.to_le_bytes());
        buf.extend_from_slice(&header.source_et_s.to_le_bytes());
        let count_offset = buf.len();
        buf.extend_from_slice(&0u16.to_le_bytes());
        Self {
            buf,
            count_offset,
            count: 0,
        }
    }

    /// Appends one record. Non-valid samples are encoded without a value (type NONE) regardless of the
    /// value they carry, so an INVALID/STALE value can never be mistaken for a measurement downstream.
    pub fn push(&mut self, channel: ChannelId, sample: &Sample) -> Result<(), CodecError> {
        if self.count >= MAX_RECORDS {
            return Err(CodecError::TooManyRecords(self.count + 1));
        }
        let value = if sample.quality == Quality::Valid {
            &sample.value
        } else {
            &Value::None
        };
        if let Value::Str(s) = value
            && s.len() > MAX_STR_VALUE_BYTES
        {
            return Err(CodecError::StringTooLong(s.len()));
        }
        let quality = if sample.quality == Quality::Valid && *value == Value::None {
            // A "valid" sample with no value is contradictory; encode it as unavailable.
            Quality::Unavailable
        } else {
            sample.quality
        };
        self.buf.extend_from_slice(&channel.0.to_le_bytes());
        let tag = ((quality as u8) << QUALITY_SHIFT) | (value.value_type() as u8);
        self.buf.push(tag);
        match value {
            Value::None => {}
            Value::F32(v) => self.buf.extend_from_slice(&v.to_le_bytes()),
            Value::F64(v) => self.buf.extend_from_slice(&v.to_le_bytes()),
            Value::I32(v) => self.buf.extend_from_slice(&v.to_le_bytes()),
            Value::U32(v) => self.buf.extend_from_slice(&v.to_le_bytes()),
            Value::Bool(v) => self.buf.push(u8::from(*v)),
            Value::U8(v) => self.buf.push(*v),
            Value::I8(v) => self.buf.push(v.to_le_bytes()[0]),
            Value::Str(s) => {
                // Length checked above to fit in u8 (MAX_STR_VALUE_BYTES < 256).
                self.buf.push(s.len() as u8);
                self.buf.extend_from_slice(s.as_bytes());
            }
            Value::U64(v) => self.buf.extend_from_slice(&v.to_le_bytes()),
        }
        self.count += 1;
        Ok(())
    }

    /// Finishes the block by patching the record count. Returns the number of records written.
    pub fn finish(self) -> usize {
        let count = self.count as u16;
        self.buf[self.count_offset..self.count_offset + 2].copy_from_slice(&count.to_le_bytes());
        self.count
    }
}

/// Bounds-checked little-endian cursor.
struct Cursor<'b> {
    data: &'b [u8],
    pos: usize,
}

impl<'b> Cursor<'b> {
    fn take(&mut self, n: usize) -> Result<&'b [u8], CodecError> {
        let end = self.pos.checked_add(n).ok_or(CodecError::Truncated {
            offset: self.pos,
            needed: n,
        })?;
        if end > self.data.len() {
            return Err(CodecError::Truncated {
                offset: self.pos,
                needed: end - self.data.len(),
            });
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        let s = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.array::<1>()?[0])
    }
}

/// Decodes a complete TLV block. Unknown channel ids are returned as-is (the caller decides whether to
/// skip them); unknown value types or qualities are errors because the record length would be unknown.
pub fn decode(data: &[u8]) -> Result<(TlvHeader, Vec<TlvRecord>), CodecError> {
    let mut c = Cursor { data, pos: 0 };
    let header = TlvHeader {
        schema_version: u16::from_le_bytes(c.array()?),
        capture_mono_ns: u64::from_le_bytes(c.array()?),
        capture_relay_ns_est: i64::from_le_bytes(c.array()?),
        source_et_s: f64::from_le_bytes(c.array()?),
    };
    let count = u16::from_le_bytes(c.array()?) as usize;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let channel = ChannelId(u16::from_le_bytes(c.array()?));
        let tag = c.u8()?;
        let vt_raw = tag & VALUE_TYPE_MASK;
        let q_raw = tag >> QUALITY_SHIFT;
        let vt = ValueType::from_u8(vt_raw).ok_or(CodecError::UnknownValueType(vt_raw))?;
        let quality = Quality::from_u8(q_raw).ok_or(CodecError::UnknownQuality(q_raw))?;
        let value = match vt {
            ValueType::None => Value::None,
            ValueType::F32 => Value::F32(f32::from_le_bytes(c.array()?)),
            ValueType::F64 => Value::F64(f64::from_le_bytes(c.array()?)),
            ValueType::I32 => Value::I32(i32::from_le_bytes(c.array()?)),
            ValueType::U32 => Value::U32(u32::from_le_bytes(c.array()?)),
            ValueType::Bool => Value::Bool(c.u8()? != 0),
            ValueType::U8 => Value::U8(c.u8()?),
            ValueType::I8 => Value::I8(i8::from_le_bytes(c.array()?)),
            ValueType::Str => {
                let len = c.u8()? as usize;
                if len > MAX_STR_VALUE_BYTES {
                    return Err(CodecError::StringTooLong(len));
                }
                let bytes = c.take(len)?;
                Value::Str(
                    std::str::from_utf8(bytes)
                        .map_err(|_| CodecError::InvalidUtf8)?
                        .to_owned(),
                )
            }
            ValueType::U64 => Value::U64(u64::from_le_bytes(c.array()?)),
        };
        records.push(TlvRecord {
            channel,
            sample: Sample { value, quality },
        });
    }
    if c.pos != data.len() {
        return Err(CodecError::TrailingBytes(data.len() - c.pos));
    }
    Ok((header, records))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> TlvHeader {
        TlvHeader {
            schema_version: 1,
            capture_mono_ns: 123_456_789,
            capture_relay_ns_est: RELAY_NS_UNKNOWN,
            source_et_s: 812.25,
        }
    }

    #[test]
    fn roundtrip_all_types() {
        let samples = vec![
            (ChannelId(1), Sample::valid(Value::F32(71.5))),
            (ChannelId(2), Sample::valid(Value::F64(-3.25))),
            (ChannelId(3), Sample::valid(Value::I32(-1))),
            (ChannelId(4), Sample::valid(Value::U32(7))),
            (ChannelId(5), Sample::valid(Value::Bool(true))),
            (ChannelId(6), Sample::valid(Value::U8(250))),
            (ChannelId(7), Sample::valid(Value::I8(-5))),
            (
                ChannelId(8),
                Sample::valid(Value::Str("Porsche 963".into())),
            ),
            (ChannelId(9), Sample::valid(Value::U64(u64::MAX))),
            (ChannelId(10), Sample::unavailable()),
            (ChannelId(11), Sample::not_supported()),
        ];
        let mut buf = Vec::new();
        let mut enc = TlvEncoder::begin(&mut buf, &header());
        for (id, s) in &samples {
            enc.push(*id, s).expect("push");
        }
        assert_eq!(enc.finish(), samples.len());
        let (h, recs) = decode(&buf).expect("decode");
        assert_eq!(h, header());
        assert_eq!(recs.len(), samples.len());
        for ((id, s), r) in samples.iter().zip(&recs) {
            assert_eq!(r.channel, *id);
            assert_eq!(&r.sample, s);
        }
    }

    #[test]
    fn invalid_sample_never_carries_value() {
        let mut buf = Vec::new();
        let mut enc = TlvEncoder::begin(&mut buf, &header());
        enc.push(
            ChannelId(1),
            &Sample {
                value: Value::F32(999.0),
                quality: Quality::Invalid,
            },
        )
        .expect("push");
        enc.finish();
        let (_, recs) = decode(&buf).expect("decode");
        assert_eq!(recs[0].sample, Sample::invalid());
    }

    #[test]
    fn truncation_is_detected_at_every_length() {
        let mut buf = Vec::new();
        let mut enc = TlvEncoder::begin(&mut buf, &header());
        enc.push(ChannelId(1), &Sample::valid(Value::F64(1.0)))
            .expect("push");
        enc.push(ChannelId(2), &Sample::valid(Value::Str("abc".into())))
            .expect("push");
        enc.finish();
        for len in 0..buf.len() {
            assert!(
                decode(&buf[..len]).is_err(),
                "prefix of length {len} must not decode"
            );
        }
        let mut extra = buf.clone();
        extra.push(0);
        assert_eq!(decode(&extra), Err(CodecError::TrailingBytes(1)));
    }

    #[test]
    fn rejects_unknown_type_and_long_strings() {
        let mut buf = Vec::new();
        TlvEncoder::begin(&mut buf, &header()).finish();
        // Patch in one record with an undefined value type 31.
        buf[TLV_HEADER_BYTES - 2..TLV_HEADER_BYTES].copy_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&[1, 0, 31]);
        assert_eq!(decode(&buf), Err(CodecError::UnknownValueType(31)));

        let mut buf2 = Vec::new();
        let mut enc = TlvEncoder::begin(&mut buf2, &header());
        let long = "x".repeat(MAX_STR_VALUE_BYTES + 1);
        assert_eq!(
            enc.push(ChannelId(1), &Sample::valid(Value::Str(long))),
            Err(CodecError::StringTooLong(MAX_STR_VALUE_BYTES + 1))
        );
    }
}

//! Golden vectors shared by the Rust model and the TypeScript decoders (D-023 §4).
//!
//! - `golden/tlv/*.bin`: TLV blocks produced by `telemetry_model::codec::TlvEncoder`, plus hand-made
//!   malformed blocks for every decode error;
//! - `golden/uibatch/*.bin`: UiBatch v1 envelopes written by [`crate::uibatch::encode_vector`]
//!   (`telemetry_model::uibatch::encode`), plus malformed envelopes for every structural error; a batch over the 4 MiB limit is described in the manifest by its
//!   length and fill byte instead of being committed (`uiBatchSynthetic`);
//! - `golden/manifest.json`: for every file, the result Rust produces — `codec::decode` for TLV blocks, the
//!   reference envelope parser for batches, and `TelemetryFrame::apply_tlv` for what applying a batch must
//!   do. Expected values are never typed by hand: they are computed from the Rust implementation, and every
//!   vector additionally declares the outcome it was built to exercise, which is checked here.
//!
//! JSON encoding: f32/f64 as hex bit patterns (so NaN, ±Inf and -0 survive), u64/i64 as decimal strings.

use std::collections::BTreeSet;

use serde::Serialize;
use telemetry_model::codec::{
    self, CodecError, RELAY_NS_UNKNOWN, TLV_HEADER_BYTES, TlvEncoder, TlvHeader, TlvRecord,
};
use telemetry_model::frame::{FrameError, TelemetryFrame};
use telemetry_model::registry;
use telemetry_model::value::MAX_STR_VALUE_BYTES;
use telemetry_model::{ChannelId, Quality, Sample, TELEMETRY_SCHEMA_VERSION, Value, ValueType, ch};

use crate::OutputFile;
use crate::uibatch::{
    self, FLAG_BACKFILL, FLAG_GAP_BEFORE, FLAG_RESET, HEADER_BYTES, MAGIC, MAX_BATCH_BYTES,
    SequenceState, UiBatchError, VERSION,
};

/// Directory of the golden files, relative to the repository root.
pub const GOLDEN_DIR: &str = "apps/desktop/src/generated/golden";

/// Driver monotonic time of ET 0 in the synthetic headers (ns).
const BASE_MONO_NS: u64 = 1_234_567_890_123;
/// Relay clock estimate of ET 0 in the synthetic headers (ns since the Unix epoch).
const BASE_RELAY_NS: i64 = 1_759_363_200_123_456_789;
const NS_PER_S: f64 = 1.0e9;

// ---------------------------------------------------------------------------------------------------------
// Manifest JSON
// ---------------------------------------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    generated: &'static str,
    telemetry_schema_version: u16,
    ui_batch_format: UiBatchFormat,
    notes: Vec<&'static str>,
    tlv: Vec<TlvVector>,
    ui_batch: Vec<UiBatchVector>,
    ui_batch_synthetic: Vec<SyntheticBatchVector>,
    ui_batch_sequences: Vec<SequenceVector>,
}

/// A batch too large to commit as a file (the TooLarge limit is 4 MiB): `length` bytes, every one equal to
/// `fill`. The expectation is the reference parser's result on exactly those bytes.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyntheticBatchVector {
    name: &'static str,
    description: &'static str,
    length: usize,
    fill: u8,
    expect: UiBatchExpect,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiBatchFormat {
    magic: String,
    version: u16,
    header_bytes: usize,
    flag_reset: u16,
    flag_backfill: u16,
    flag_gap_before: u16,
    max_batch_bytes: usize,
}

#[derive(Serialize)]
struct TlvVector {
    name: &'static str,
    file: String,
    description: &'static str,
    expect: TlvExpect,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum TlvExpect {
    Ok {
        header: JsonHeader,
        records: Vec<JsonRecord>,
    },
    Error(JsonCodecError),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonHeader {
    schema_version: u16,
    capture_mono_ns: String,
    capture_relay_ns_est: String,
    source_et_s: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonRecord {
    channel: u16,
    quality: u8,
    value: JsonValue,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum JsonValue {
    None,
    F32 { bits: String },
    F64 { bits: String },
    I32 { value: i32 },
    U32 { value: u32 },
    Bool { value: bool },
    U8 { value: u8 },
    I8 { value: i8 },
    Str { value: String, utf8: String },
    U64 { value: String },
}

#[derive(Serialize)]
#[serde(tag = "kind")]
enum JsonCodecError {
    Truncated {
        offset: usize,
        needed: usize,
    },
    UnknownValueType {
        #[serde(rename = "valueType")]
        value_type: u8,
    },
    UnknownQuality {
        quality: u8,
    },
    StringTooLong {
        length: usize,
    },
    InvalidUtf8,
    TooManyRecords {
        count: usize,
    },
    TrailingBytes {
        count: usize,
    },
}

#[derive(Serialize)]
struct UiBatchVector {
    name: &'static str,
    file: String,
    description: &'static str,
    expect: UiBatchExpect,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum UiBatchExpect {
    Ok(UiBatchOk),
    Error(JsonUiBatchError),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiBatchOk {
    version: u16,
    flags: u16,
    reset: bool,
    backfill: bool,
    gap_before: bool,
    batch_seq: u32,
    blocks: Vec<UiBlock>,
    apply: ApplyExpect,
}

#[derive(Serialize)]
struct UiBlock {
    offset: usize,
    length: usize,
    decode: TlvExpect,
}

/// What applying the batch to a store must do (mirrors `TelemetryFrame::apply_tlv` per block).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum ApplyExpect {
    Accepted(FrameExpect),
    Rejected(RejectExpect),
}

/// Result of applying every block, in order, to a fresh frame: the summed `DecodeReport` and the final
/// sample of every registry channel a record of the batch touched.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FrameExpect {
    applied: usize,
    unknown_skipped: usize,
    invalidated: usize,
    samples: Vec<JsonRecord>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RejectExpect {
    block_index: usize,
    reason: RejectReason,
}

#[derive(Serialize)]
#[serde(tag = "kind")]
enum RejectReason {
    Codec { error: JsonCodecError },
    SchemaMismatch { found: u16, expected: u16 },
}

#[derive(Serialize)]
#[serde(tag = "kind")]
enum JsonUiBatchError {
    TooLarge { length: usize, max: usize },
    Truncated { offset: usize, needed: usize },
    BadMagic { found: String },
    UnsupportedVersion { version: u16 },
    UnknownFlags { bits: u16 },
    ReservedNonZero { value: u16 },
    TrailingBytes { count: usize },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SequenceVector {
    name: &'static str,
    description: &'static str,
    files: Vec<String>,
    gap_before: Vec<bool>,
}

// ---------------------------------------------------------------------------------------------------------
// JSON conversions
// ---------------------------------------------------------------------------------------------------------

fn hex_f32(v: f32) -> String {
    format!("0x{:08x}", v.to_bits())
}

fn hex_f64(v: f64) -> String {
    format!("0x{:016x}", v.to_bits())
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn json_value(value: &Value) -> JsonValue {
    match value {
        Value::None => JsonValue::None,
        Value::F32(v) => JsonValue::F32 { bits: hex_f32(*v) },
        Value::F64(v) => JsonValue::F64 { bits: hex_f64(*v) },
        Value::I32(v) => JsonValue::I32 { value: *v },
        Value::U32(v) => JsonValue::U32 { value: *v },
        Value::Bool(v) => JsonValue::Bool { value: *v },
        Value::U8(v) => JsonValue::U8 { value: *v },
        Value::I8(v) => JsonValue::I8 { value: *v },
        Value::Str(s) => JsonValue::Str {
            value: s.clone(),
            utf8: hex_bytes(s.as_bytes()),
        },
        Value::U64(v) => JsonValue::U64 {
            value: v.to_string(),
        },
    }
}

fn json_record(channel: ChannelId, sample: &Sample) -> JsonRecord {
    JsonRecord {
        channel: channel.0,
        quality: sample.quality as u8,
        value: json_value(&sample.value),
    }
}

fn json_codec_error(e: &CodecError) -> JsonCodecError {
    match *e {
        CodecError::Truncated { offset, needed } => JsonCodecError::Truncated { offset, needed },
        CodecError::UnknownValueType(value_type) => JsonCodecError::UnknownValueType { value_type },
        CodecError::UnknownQuality(quality) => JsonCodecError::UnknownQuality { quality },
        CodecError::StringTooLong(length) => JsonCodecError::StringTooLong { length },
        CodecError::InvalidUtf8 => JsonCodecError::InvalidUtf8,
        CodecError::TooManyRecords(count) => JsonCodecError::TooManyRecords { count },
        CodecError::TrailingBytes(count) => JsonCodecError::TrailingBytes { count },
    }
}

fn codec_error_kind(e: &CodecError) -> &'static str {
    match e {
        CodecError::Truncated { .. } => "Truncated",
        CodecError::UnknownValueType(_) => "UnknownValueType",
        CodecError::UnknownQuality(_) => "UnknownQuality",
        CodecError::StringTooLong(_) => "StringTooLong",
        CodecError::InvalidUtf8 => "InvalidUtf8",
        CodecError::TooManyRecords(_) => "TooManyRecords",
        CodecError::TrailingBytes(_) => "TrailingBytes",
    }
}

fn tlv_expect(bytes: &[u8]) -> TlvExpect {
    match codec::decode(bytes) {
        Ok((header, records)) => TlvExpect::Ok {
            header: JsonHeader {
                schema_version: header.schema_version,
                capture_mono_ns: header.capture_mono_ns.to_string(),
                capture_relay_ns_est: header.capture_relay_ns_est.to_string(),
                source_et_s: hex_f64(header.source_et_s),
            },
            records: records
                .iter()
                .map(|r: &TlvRecord| json_record(r.channel, &r.sample))
                .collect(),
        },
        Err(e) => TlvExpect::Error(json_codec_error(&e)),
    }
}

fn json_ui_error(e: &UiBatchError) -> JsonUiBatchError {
    match *e {
        UiBatchError::TooLarge { length, max } => JsonUiBatchError::TooLarge { length, max },
        UiBatchError::Truncated { offset, needed } => {
            JsonUiBatchError::Truncated { offset, needed }
        }
        UiBatchError::BadMagic { found } => JsonUiBatchError::BadMagic {
            found: hex_bytes(&found),
        },
        UiBatchError::UnsupportedVersion { version } => {
            JsonUiBatchError::UnsupportedVersion { version }
        }
        UiBatchError::UnknownFlags { bits } => JsonUiBatchError::UnknownFlags { bits },
        UiBatchError::ReservedNonZero { value } => JsonUiBatchError::ReservedNonZero { value },
        UiBatchError::TrailingBytes { count } => JsonUiBatchError::TrailingBytes { count },
    }
}

/// Applies every block of a structurally valid batch to a fresh frame, as a store must.
fn apply_expect(batch: &[u8], parsed: &uibatch::ParsedBatch) -> ApplyExpect {
    let mut frame = TelemetryFrame::new(0, 0, None);
    let (mut applied, mut unknown_skipped, mut invalidated) = (0, 0, 0);
    let mut touched = BTreeSet::new();
    for (index, &(offset, length)) in parsed.blocks.iter().enumerate() {
        let block = &batch[offset..offset + length];
        match frame.apply_tlv(u64::try_from(index).unwrap_or(u64::MAX), block) {
            Ok((next, report)) => {
                frame = next;
                applied += report.applied;
                unknown_skipped += report.unknown_skipped;
                invalidated += report.invalidated;
                if let Ok((_, records)) = codec::decode(block) {
                    for r in records {
                        if let Some(i) = registry::index_of(r.channel) {
                            touched.insert(i);
                        }
                    }
                }
            }
            Err(FrameError::Codec(error)) => {
                return ApplyExpect::Rejected(RejectExpect {
                    block_index: index,
                    reason: RejectReason::Codec {
                        error: json_codec_error(&error),
                    },
                });
            }
            Err(FrameError::SchemaMismatch { found, expected }) => {
                return ApplyExpect::Rejected(RejectExpect {
                    block_index: index,
                    reason: RejectReason::SchemaMismatch { found, expected },
                });
            }
            Err(other) => panic!("apply_tlv returned an unexpected error: {other}"),
        }
    }
    let samples = touched
        .into_iter()
        .filter_map(|i| registry::CHANNELS.get(i))
        .map(|def| json_record(def.id, frame.get(def.id)))
        .collect();
    ApplyExpect::Accepted(FrameExpect {
        applied,
        unknown_skipped,
        invalidated,
        samples,
    })
}

// ---------------------------------------------------------------------------------------------------------
// Block construction
// ---------------------------------------------------------------------------------------------------------

/// Header of a sample captured at session time `et_s` (monotonic and relay clocks advance with ET).
fn header(et_s: f64) -> TlvHeader {
    let offset_ns = if et_s.is_finite() {
        (et_s * NS_PER_S).round()
    } else {
        0.0
    };
    // Synthetic ETs are small and positive, so the offsets fit both clocks exactly.
    let offset_u = offset_ns as u64;
    let offset_i = offset_ns as i64;
    TlvHeader {
        schema_version: TELEMETRY_SCHEMA_VERSION,
        capture_mono_ns: BASE_MONO_NS + offset_u,
        capture_relay_ns_est: BASE_RELAY_NS + offset_i,
        source_et_s: et_s,
    }
}

/// A block written by the real encoder.
fn encode(header: &TlvHeader, records: &[(ChannelId, Sample)]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = TlvEncoder::begin(&mut buf, header);
    for (id, sample) in records {
        enc.push(*id, sample).expect("golden record must encode");
    }
    enc.finish();
    buf
}

/// A block whose header announces `count` records followed by the raw `body` bytes.
fn raw(header: &TlvHeader, count: u16, body: &[u8]) -> Vec<u8> {
    let mut buf = encode(header, &[]);
    buf[TLV_HEADER_BYTES - 2..TLV_HEADER_BYTES].copy_from_slice(&count.to_le_bytes());
    buf.extend_from_slice(body);
    buf
}

/// A raw TLV tag byte (quality in the high 3 bits, value type in the low 5).
const fn tag(quality: u8, value_type: u8) -> u8 {
    (quality << 5) | value_type
}

/// Raw record bytes: channel id, tag, value bytes.
fn raw_record(id: ChannelId, quality: u8, value_type: u8, value: &[u8]) -> Vec<u8> {
    let mut out = id.0.to_le_bytes().to_vec();
    out.push(tag(quality, value_type));
    out.extend_from_slice(value);
    out
}

/// The `nth` registry channel of value type `vt` (so typed records match the registry).
fn channel_of(vt: ValueType, nth: usize) -> ChannelId {
    registry::all()
        .iter()
        .filter(|d| d.value_type == vt)
        .nth(nth)
        .map(|d| d.id)
        .unwrap_or_else(|| panic!("registry has fewer than {} {vt:?} channels", nth + 1))
}

fn valid(value: Value) -> Sample {
    Sample::valid(value)
}

fn with_quality(value: Value, quality: Quality) -> Sample {
    Sample { value, quality }
}

const Q_VALID: u8 = Quality::Valid as u8;
const Q_INVALID: u8 = Quality::Invalid as u8;
const Q_STALE: u8 = Quality::Stale as u8;
const Q_NOT_SUPPORTED: u8 = Quality::NotSupported as u8;
const T_NONE: u8 = ValueType::None as u8;
const T_F32: u8 = ValueType::F32 as u8;
const T_F64: u8 = ValueType::F64 as u8;
const T_BOOL: u8 = ValueType::Bool as u8;
const T_U8: u8 = ValueType::U8 as u8;
const T_STR: u8 = ValueType::Str as u8;
const T_U64: u8 = ValueType::U64 as u8;
/// First value type code without a meaning.
const T_FIRST_UNASSIGNED: u8 = ValueType::U64 as u8 + 1;
/// Largest value type code the 5-bit field can hold.
const T_MAX_CODE: u8 = 0b1_1111;
/// First quality code without a meaning.
const Q_FIRST_UNASSIGNED: u8 = Quality::NotSupported as u8 + 1;
/// Largest quality code the 3-bit field can hold.
const Q_MAX_CODE: u8 = 0b111;
/// An id outside every registry group.
const UNKNOWN_ID: ChannelId = ChannelId(0x0EFF);

// ---------------------------------------------------------------------------------------------------------
// Collector
// ---------------------------------------------------------------------------------------------------------

/// What a vector was built to exercise; checked against the computed result.
enum Intended {
    Ok,
    Err(&'static str),
}

#[derive(Default)]
struct Golden {
    files: Vec<OutputFile>,
    tlv: Vec<TlvVector>,
    ui: Vec<UiBatchVector>,
    synthetic: Vec<SyntheticBatchVector>,
    sequences: Vec<SequenceVector>,
    batches: Vec<(&'static str, Vec<u8>)>,
}

/// Panics unless `parsed` is the outcome the vector `name` was built to exercise.
fn check_intended(
    name: &str,
    intended: &Intended,
    parsed: &Result<uibatch::ParsedBatch, UiBatchError>,
) {
    match (intended, parsed) {
        (Intended::Ok, Ok(_)) => {}
        (Intended::Err(kind), Err(e)) if e.kind() == *kind => {}
        _ => panic!(
            "UiBatch vector {name}: built for {:?}, parses to {parsed:?}",
            match intended {
                Intended::Ok => "Ok",
                Intended::Err(kind) => kind,
            }
        ),
    }
}

/// Expected outcome of a batch: the reference parser's result, and for a valid envelope the decode of every
/// block and what applying the batch must do.
fn ui_batch_expect(
    bytes: &[u8],
    parsed: &Result<uibatch::ParsedBatch, UiBatchError>,
) -> UiBatchExpect {
    match parsed {
        Ok(p) => UiBatchExpect::Ok(UiBatchOk {
            version: p.version,
            flags: p.flags,
            reset: p.flags & FLAG_RESET != 0,
            backfill: p.flags & FLAG_BACKFILL != 0,
            gap_before: p.flags & FLAG_GAP_BEFORE != 0,
            batch_seq: p.batch_seq,
            blocks: p
                .blocks
                .iter()
                .map(|&(offset, length)| UiBlock {
                    offset,
                    length,
                    decode: tlv_expect(&bytes[offset..offset + length]),
                })
                .collect(),
            apply: apply_expect(bytes, p),
        }),
        Err(e) => UiBatchExpect::Error(json_ui_error(e)),
    }
}

impl Golden {
    fn tlv(
        &mut self,
        name: &'static str,
        description: &'static str,
        bytes: Vec<u8>,
        intended: Intended,
    ) {
        let result = codec::decode(&bytes);
        match (&intended, &result) {
            (Intended::Ok, Ok(_)) => {}
            (Intended::Err(kind), Err(e)) if codec_error_kind(e) == *kind => {}
            _ => panic!(
                "TLV vector {name}: built for {:?}, decodes to {result:?}",
                match intended {
                    Intended::Ok => "Ok",
                    Intended::Err(kind) => kind,
                }
            ),
        }
        let file = format!("tlv/{name}.bin");
        self.tlv.push(TlvVector {
            name,
            file: file.clone(),
            description,
            expect: tlv_expect(&bytes),
        });
        self.files.push(OutputFile {
            path: format!("{GOLDEN_DIR}/{file}"),
            contents: bytes,
        });
    }

    fn batch(
        &mut self,
        name: &'static str,
        description: &'static str,
        bytes: Vec<u8>,
        intended: Intended,
    ) {
        let parsed = uibatch::parse(&bytes);
        check_intended(name, &intended, &parsed);
        let expect = ui_batch_expect(&bytes, &parsed);
        let file = format!("uibatch/{name}.bin");
        self.ui.push(UiBatchVector {
            name,
            file: file.clone(),
            description,
            expect,
        });
        self.batches.push((name, bytes.clone()));
        self.files.push(OutputFile {
            path: format!("{GOLDEN_DIR}/{file}"),
            contents: bytes,
        });
    }

    /// A vector described by its length and fill byte instead of a file (see [`SyntheticBatchVector`]).
    fn synthetic_batch(
        &mut self,
        name: &'static str,
        description: &'static str,
        (length, fill): (usize, u8),
        intended: Intended,
    ) {
        let bytes = vec![fill; length];
        let parsed = uibatch::parse(&bytes);
        check_intended(name, &intended, &parsed);
        self.synthetic.push(SyntheticBatchVector {
            name,
            description,
            length,
            fill,
            expect: ui_batch_expect(&bytes, &parsed),
        });
    }

    fn sequence(
        &mut self,
        name: &'static str,
        description: &'static str,
        batch_names: &[&'static str],
    ) {
        let mut state = SequenceState::default();
        let mut gap_before = Vec::new();
        for batch_name in batch_names {
            let bytes = self
                .batches
                .iter()
                .find(|(n, _)| n == batch_name)
                .map(|(_, b)| b.as_slice())
                .unwrap_or_else(|| panic!("sequence {name}: unknown batch {batch_name}"));
            let parsed = uibatch::parse(bytes).unwrap_or_else(|e| {
                panic!("sequence {name}: batch {batch_name} is invalid: {e:?}")
            });
            gap_before.push(state.observe(parsed.flags, parsed.batch_seq));
        }
        self.sequences.push(SequenceVector {
            name,
            description,
            files: batch_names
                .iter()
                .map(|n| format!("uibatch/{n}.bin"))
                .collect(),
            gap_before,
        });
    }
}

// ---------------------------------------------------------------------------------------------------------
// TLV vectors
// ---------------------------------------------------------------------------------------------------------

fn tlv_vectors(g: &mut Golden) {
    let h = header(812.25);

    g.tlv(
        "empty_block",
        "A block with no records.",
        encode(&h, &[]),
        Intended::Ok,
    );
    g.tlv(
        "empty_block_unknown_clocks",
        "No records; relay clock estimate unknown (i64::MIN), source ET unavailable (NaN), monotonic 0.",
        encode(
            &TlvHeader {
                schema_version: TELEMETRY_SCHEMA_VERSION,
                capture_mono_ns: 0,
                capture_relay_ns_est: RELAY_NS_UNKNOWN,
                source_et_s: f64::NAN,
            },
            &[],
        ),
        Intended::Ok,
    );
    g.tlv(
        "header_extremes",
        "Header limits: monotonic u64::MAX, relay i64::MAX, ET -0.0.",
        encode(
            &TlvHeader {
                schema_version: TELEMETRY_SCHEMA_VERSION,
                capture_mono_ns: u64::MAX,
                capture_relay_ns_est: i64::MAX,
                source_et_s: -0.0,
            },
            &[],
        ),
        Intended::Ok,
    );
    g.tlv(
        "header_extremes_negative",
        "Header limits: monotonic 1, relay i64::MIN + 1 (not the sentinel), ET +Infinity.",
        encode(
            &TlvHeader {
                schema_version: TELEMETRY_SCHEMA_VERSION,
                capture_mono_ns: 1,
                capture_relay_ns_est: i64::MIN + 1,
                source_et_s: f64::INFINITY,
            },
            &[],
        ),
        Intended::Ok,
    );
    g.tlv(
        "schema_version_2",
        "A well-formed block announcing telemetry schema 2: the codec decodes it, a store must refuse it.",
        encode(
            &TlvHeader {
                schema_version: TELEMETRY_SCHEMA_VERSION + 1,
                ..h
            },
            &[(ch::SPEED, valid(Value::F32(42.0)))],
        ),
        Intended::Ok,
    );
    g.tlv(
        "all_value_types",
        "One VALID record of every value type on registry channels of that type, then UNAVAILABLE and \
         NOT_SUPPORTED records (type NONE).",
        encode(
            &h,
            &[
                (channel_of(ValueType::F32, 0), valid(Value::F32(71.5))),
                (channel_of(ValueType::F64, 0), valid(Value::F64(-3.25))),
                (channel_of(ValueType::I32, 0), valid(Value::I32(-1))),
                (channel_of(ValueType::U32, 0), valid(Value::U32(7))),
                (channel_of(ValueType::Bool, 0), valid(Value::Bool(true))),
                (channel_of(ValueType::Bool, 1), valid(Value::Bool(false))),
                (channel_of(ValueType::U8, 0), valid(Value::U8(250))),
                (channel_of(ValueType::I8, 0), valid(Value::I8(-5))),
                (
                    channel_of(ValueType::Str, 0),
                    valid(Value::Str("Porsche 963".into())),
                ),
                (channel_of(ValueType::U64, 0), valid(Value::U64(u64::MAX))),
                (channel_of(ValueType::F32, 1), Sample::unavailable()),
                (channel_of(ValueType::F32, 2), Sample::not_supported()),
            ],
        ),
        Intended::Ok,
    );
    g.tlv(
        "bool_nonzero_byte",
        "A BOOL record whose byte is 2: any non-zero byte decodes as true.",
        raw(
            &h,
            1,
            &raw_record(channel_of(ValueType::Bool, 0), Q_VALID, T_BOOL, &[2]),
        ),
        Intended::Ok,
    );
    g.tlv(
        "integer_extremes",
        "Integer limits; U64 values above 2^53 must keep full precision.",
        encode(
            &h,
            &[
                (ChannelId(1), valid(Value::I32(i32::MIN))),
                (ChannelId(2), valid(Value::I32(i32::MAX))),
                (ChannelId(3), valid(Value::U32(u32::MAX))),
                (ChannelId(4), valid(Value::U8(0))),
                (ChannelId(5), valid(Value::U8(u8::MAX))),
                (ChannelId(6), valid(Value::I8(i8::MIN))),
                (ChannelId(7), valid(Value::I8(i8::MAX))),
                (ChannelId(8), valid(Value::U64(0))),
                (ChannelId(9), valid(Value::U64((1 << 53) + 1))),
                (ChannelId(10), valid(Value::U64(u64::MAX))),
            ],
        ),
        Intended::Ok,
    );
    let f32s = [
        0.0,
        -0.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        f32::from_bits(0x7fc0_0001),
        f32::from_bits(1),
        f32::MIN_POSITIVE,
        f32::MAX,
        f32::MIN,
        0.1,
    ];
    let f64s = [
        0.0,
        -0.0,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        f64::from_bits(0x7ff8_0000_0000_0001),
        f64::from_bits(1),
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        0.1,
        std::f64::consts::PI,
    ];
    let mut floats: Vec<(ChannelId, Sample)> = Vec::new();
    for (i, v) in f32s.iter().enumerate() {
        let id = u16::try_from(i + 1).unwrap_or(u16::MAX);
        floats.push((ChannelId(id), valid(Value::F32(*v))));
    }
    for (i, v) in f64s.iter().enumerate() {
        let id = u16::try_from(i + 0x100).unwrap_or(u16::MAX);
        floats.push((ChannelId(id), valid(Value::F64(*v))));
    }
    g.tlv(
        "float_specials",
        "VALID F32/F64 records with ±0, ±Inf, NaN (with and without payload), subnormals and limits; the \
         codec carries them unchanged (a frame or store marks non-finite VALID floats INVALID).",
        encode(&h, &floats),
        Intended::Ok,
    );
    let f32_ids: Vec<ChannelId> = (0..6).map(|n| channel_of(ValueType::F32, n)).collect();
    g.tlv(
        "every_quality",
        "Every quality on F32 channels. The encoder drops the value of non-VALID samples (type NONE, no value \
         bytes) and encodes a VALID sample without a value as UNAVAILABLE.",
        encode(
            &h,
            &[
                (f32_ids[0], valid(Value::F32(1.5))),
                (f32_ids[1], with_quality(Value::F32(999.0), Quality::Invalid)),
                (f32_ids[2], with_quality(Value::F32(999.0), Quality::Stale)),
                (f32_ids[3], Sample::unavailable()),
                (f32_ids[4], Sample::not_supported()),
                (f32_ids[5], with_quality(Value::None, Quality::Valid)),
            ],
        ),
        Intended::Ok,
    );
    let mut non_valid = raw_record(
        channel_of(ValueType::F32, 0),
        Q_INVALID,
        T_F32,
        &999.0f32.to_le_bytes(),
    );
    non_valid.extend(raw_record(
        channel_of(ValueType::U8, 0),
        Q_STALE,
        T_U8,
        &[42],
    ));
    non_valid.extend(raw_record(
        channel_of(ValueType::Str, 0),
        Q_NOT_SUPPORTED,
        T_STR,
        &[2, b'o', b'k'],
    ));
    g.tlv(
        "non_valid_with_value_bytes",
        "Hand-made: non-VALID records that still carry value bytes. The codec decodes the values as-is; a \
         frame or store drops them.",
        raw(&h, 3, &non_valid),
        Intended::Ok,
    );
    let max_ascii = "x".repeat(MAX_STR_VALUE_BYTES);
    let max_four_byte = "\u{1F3C1}".repeat(MAX_STR_VALUE_BYTES / 4);
    let max_two_byte = "\u{e9}".repeat(MAX_STR_VALUE_BYTES / 2);
    let strings = [
        String::new(),
        "Porsche 963".to_owned(),
        "\u{dc}".to_owned(),
        "\u{65e5}\u{672c}\u{8a9e}".to_owned(),
        "\u{1F3C1}".to_owned(),
        "\u{feff}BOM".to_owned(),
        "a\u{0}b".to_owned(),
        "\u{10FFFF}".to_owned(),
        max_ascii,
        max_four_byte,
        max_two_byte,
    ];
    let records: Vec<(ChannelId, Sample)> = strings
        .iter()
        .enumerate()
        .map(|(n, s)| (channel_of(ValueType::Str, n), valid(Value::Str(s.clone()))))
        .collect();
    g.tlv(
        "strings",
        "STR values: empty, ASCII, 2/3/4-byte UTF-8, a leading U+FEFF (must be kept), an embedded NUL, \
         U+10FFFF, and the 64-byte maximum in 1-, 4- and 2-byte characters.",
        encode(&h, &records),
        Intended::Ok,
    );
    g.tlv(
        "unknown_channel_ids",
        "Channel ids the registry does not define (0, 0xFFFF, outside every group): the codec returns them; \
         a frame or store skips them.",
        encode(
            &h,
            &[
                (ChannelId(0), valid(Value::F32(1.0))),
                (ChannelId(u16::MAX), valid(Value::U8(1))),
                (UNKNOWN_ID, valid(Value::Str("future".into()))),
            ],
        ),
        Intended::Ok,
    );

    // ----- decode errors -----
    let empty = encode(&h, &[]);
    let id = channel_of(ValueType::F32, 0);
    let id_bytes = id.0.to_le_bytes();
    g.tlv(
        "err_truncated_header_empty",
        "Zero bytes.",
        Vec::new(),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_header_mid_field",
        "13 bytes: the header ends inside capture_relay_ns_est.",
        empty[..13].to_vec(),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_header_count",
        "The header ends one byte into record_count.",
        empty[..TLV_HEADER_BYTES - 1].to_vec(),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_record_channel_id",
        "One record announced; only one byte of its channel id is present.",
        raw(&h, 1, &id_bytes[..1]),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_record_tag",
        "One record announced; its tag byte is missing.",
        raw(&h, 1, &id_bytes),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_value_f32",
        "F32 record with 2 of 4 value bytes.",
        raw(&h, 1, &raw_record(id, Q_VALID, T_F32, &[0, 0])),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_value_f64",
        "F64 record with 7 of 8 value bytes.",
        raw(&h, 1, &raw_record(id, Q_VALID, T_F64, &[0; 7])),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_value_u64",
        "U64 record without value bytes.",
        raw(&h, 1, &raw_record(id, Q_VALID, T_U64, &[])),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_value_bool",
        "BOOL record without its byte.",
        raw(&h, 1, &raw_record(id, Q_VALID, T_BOOL, &[])),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_string_length",
        "STR record without its length byte.",
        raw(&h, 1, &raw_record(id, Q_VALID, T_STR, &[])),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_truncated_string_bytes",
        "STR record announcing 10 bytes with 4 present.",
        raw(
            &h,
            1,
            &raw_record(id, Q_VALID, T_STR, &[10, b'a', b'b', b'c', b'd']),
        ),
        Intended::Err("Truncated"),
    );
    let mut second = encode(&h, &[(id, valid(Value::F32(1.0)))]);
    second[TLV_HEADER_BYTES - 2..TLV_HEADER_BYTES].copy_from_slice(&2u16.to_le_bytes());
    g.tlv(
        "err_truncated_second_record",
        "Two records announced, one present: the decode fails although the first record is complete.",
        second,
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_count_exceeds_bytes",
        "65535 records announced, none present (an untrusted count must not drive allocation).",
        raw(&h, u16::MAX, &[]),
        Intended::Err("Truncated"),
    );
    g.tlv(
        "err_unknown_value_type_first_unassigned",
        "Value type 10, the first code without a meaning.",
        raw(&h, 1, &raw_record(id, Q_VALID, T_FIRST_UNASSIGNED, &[])),
        Intended::Err("UnknownValueType"),
    );
    g.tlv(
        "err_unknown_value_type_max",
        "Value type 31, the largest 5-bit code.",
        raw(&h, 1, &raw_record(id, Q_VALID, T_MAX_CODE, &[])),
        Intended::Err("UnknownValueType"),
    );
    g.tlv(
        "err_unknown_value_type_before_quality",
        "Unknown value type and unknown quality in one tag: the value type is checked first.",
        raw(&h, 1, &raw_record(id, Q_MAX_CODE, T_MAX_CODE, &[])),
        Intended::Err("UnknownValueType"),
    );
    g.tlv(
        "err_unknown_quality_first_unassigned",
        "Quality 5, the first code without a meaning.",
        raw(&h, 1, &raw_record(id, Q_FIRST_UNASSIGNED, T_NONE, &[])),
        Intended::Err("UnknownQuality"),
    );
    g.tlv(
        "err_unknown_quality_max",
        "Quality 7 on an F32 record (checked before the value is read).",
        raw(&h, 1, &raw_record(id, Q_MAX_CODE, T_F32, &[0; 4])),
        Intended::Err("UnknownQuality"),
    );
    let mut after_valid = raw_record(id, Q_VALID, T_F32, &1.0f32.to_le_bytes());
    after_valid.extend(raw_record(id, Q_FIRST_UNASSIGNED + 1, T_NONE, &[]));
    g.tlv(
        "err_unknown_quality_after_valid_record",
        "A valid record followed by a record with quality 6: the whole block fails.",
        raw(&h, 2, &after_valid),
        Intended::Err("UnknownQuality"),
    );
    let too_long = u8::try_from(MAX_STR_VALUE_BYTES + 1).unwrap_or(u8::MAX);
    g.tlv(
        "err_string_too_long_by_one",
        "STR length byte 65 (one over the maximum); rejected before the bytes are read.",
        raw(&h, 1, &raw_record(id, Q_VALID, T_STR, &[too_long])),
        Intended::Err("StringTooLong"),
    );
    g.tlv(
        "err_string_too_long_max_byte",
        "STR length byte 255.",
        raw(&h, 1, &raw_record(id, Q_VALID, T_STR, &[u8::MAX])),
        Intended::Err("StringTooLong"),
    );
    let utf8_cases: [(&'static str, &'static str, &[u8]); 5] = [
        (
            "err_invalid_utf8_lone_continuation",
            "STR bytes 0x80 (a continuation byte without a lead byte).",
            &[0x80],
        ),
        (
            "err_invalid_utf8_overlong",
            "STR bytes C0 AF (overlong encoding of '/').",
            &[0xC0, 0xAF],
        ),
        (
            "err_invalid_utf8_surrogate",
            "STR bytes ED A0 80 (an encoded UTF-16 surrogate).",
            &[0xED, 0xA0, 0x80],
        ),
        (
            "err_invalid_utf8_above_max",
            "STR bytes F4 90 80 80 (above U+10FFFF).",
            &[0xF4, 0x90, 0x80, 0x80],
        ),
        (
            "err_invalid_utf8_incomplete",
            "STR bytes 'a' E6 97 (a 3-byte sequence cut short by the string length).",
            &[b'a', 0xE6, 0x97],
        ),
    ];
    for (name, description, bytes) in utf8_cases {
        let mut value = vec![u8::try_from(bytes.len()).unwrap_or(u8::MAX)];
        value.extend_from_slice(bytes);
        g.tlv(
            name,
            description,
            raw(&h, 1, &raw_record(id, Q_VALID, T_STR, &value)),
            Intended::Err("InvalidUtf8"),
        );
    }
    let mut trailing_one = encode(&h, &[(id, valid(Value::F32(1.0)))]);
    trailing_one.push(0);
    g.tlv(
        "err_trailing_bytes_one",
        "A complete block followed by one extra byte.",
        trailing_one,
        Intended::Err("TrailingBytes"),
    );
    let mut trailing_five = empty;
    trailing_five.extend_from_slice(&[1, 2, 3, 4, 5]);
    g.tlv(
        "err_trailing_bytes_after_empty_block",
        "An empty block followed by five extra bytes.",
        trailing_five,
        Intended::Err("TrailingBytes"),
    );
}

// ---------------------------------------------------------------------------------------------------------
// UiBatch vectors
// ---------------------------------------------------------------------------------------------------------

fn ui_batch_vectors(g: &mut Golden) {
    let a = encode(
        &header(100.0),
        &[
            (ch::SPEED, valid(Value::F32(50.0))),
            (ch::GEAR, valid(Value::I32(3))),
            (ch::RPM, valid(Value::F32(8_000.0))),
            (ch::VEHICLE_NAME, valid(Value::Str("Porsche 963 #6".into()))),
        ],
    );
    let b = encode(
        &header(100.01),
        &[
            (ch::SPEED, valid(Value::F32(50.5))),
            (ch::RPM, valid(Value::F32(8_050.0))),
        ],
    );
    let c = encode(
        &header(100.02),
        &[
            (ch::SPEED, valid(Value::F32(51.0))),
            (ch::GEAR, valid(Value::I32(4))),
        ],
    );
    let qualities = encode(
        &header(100.03),
        &[
            (ch::SPEED, with_quality(Value::F32(51.5), Quality::Invalid)),
            (ch::GEAR, Sample::stale()),
            (ch::RPM, Sample::unavailable()),
            (ch::TYRE_PRESSURE_FL, Sample::not_supported()),
        ],
    );
    let mut contradictions = raw_record(ch::SPEED, Q_VALID, T_F64, &52.0f64.to_le_bytes());
    contradictions.extend(raw_record(ch::RPM, Q_VALID, T_F32, &f32::NAN.to_le_bytes()));
    contradictions.extend(raw_record(ch::GEAR, Q_VALID, T_NONE, &[]));
    contradictions.extend(raw_record(
        UNKNOWN_ID,
        Q_VALID,
        T_F32,
        &1.0f32.to_le_bytes(),
    ));
    contradictions.extend(raw_record(
        ch::THROTTLE,
        Q_VALID,
        T_F32,
        &0.5f32.to_le_bytes(),
    ));
    let contradictions = raw(&header(100.04), 5, &contradictions);
    let no_et = encode(&header(f64::NAN), &[(ch::SPEED, valid(Value::F32(52.0)))]);
    let schema2 = encode(
        &TlvHeader {
            schema_version: TELEMETRY_SCHEMA_VERSION + 1,
            ..header(100.05)
        },
        &[(ch::SPEED, valid(Value::F32(53.0)))],
    );
    let truncated = a[..a.len() - 1].to_vec();

    g.batch(
        "single_block",
        "One block, no flags, batch_seq 1.",
        uibatch::encode_vector(0, 1, &[&a]),
        Intended::Ok,
    );
    g.batch(
        "multi_block",
        "Three blocks, oldest first (ET 100.00, 100.01, 100.02).",
        uibatch::encode_vector(0, 2, &[&a, &b, &c]),
        Intended::Ok,
    );
    g.batch(
        "empty_batch",
        "No blocks (valid; nothing to apply).",
        uibatch::encode_vector(0, 3, &[]),
        Intended::Ok,
    );
    g.batch(
        "reset",
        "RESET: discard cached values and ring contents, then apply.",
        uibatch::encode_vector(FLAG_RESET, 0, &[&a]),
        Intended::Ok,
    );
    g.batch(
        "reset_backfill",
        "RESET | BACKFILL: first batch of a subscription, historical blocks oldest first.",
        uibatch::encode_vector(FLAG_RESET | FLAG_BACKFILL, 0, &[&a, &b]),
        Intended::Ok,
    );
    g.batch(
        "backfill_continuation",
        "BACKFILL without RESET: a later part of a split backfill.",
        uibatch::encode_vector(FLAG_BACKFILL, 1, &[&c]),
        Intended::Ok,
    );
    g.batch(
        "gap_before",
        "GAP_BEFORE: the bridge dropped blocks before this batch.",
        uibatch::encode_vector(FLAG_GAP_BEFORE, 4, &[&c]),
        Intended::Ok,
    );
    g.batch(
        "all_flags",
        "RESET | BACKFILL | GAP_BEFORE.",
        uibatch::encode_vector(FLAG_RESET | FLAG_BACKFILL | FLAG_GAP_BEFORE, 0, &[&a]),
        Intended::Ok,
    );
    g.batch(
        "qualities",
        "A block of values followed by a block of INVALID / STALE / UNAVAILABLE / NOT_SUPPORTED records.",
        uibatch::encode_vector(0, 5, &[&a, &qualities]),
        Intended::Ok,
    );
    g.batch(
        "registry_contradictions",
        "Records that contradict the registry (wrong type, VALID NaN, VALID without value) become INVALID; \
         an unknown id is skipped; a correct record is applied.",
        uibatch::encode_vector(0, 6, &[&contradictions]),
        Intended::Ok,
    );
    g.batch(
        "et_not_finite",
        "A block without a finite source ET updates current values but never enters the ring.",
        uibatch::encode_vector(0, 7, &[&no_et]),
        Intended::Ok,
    );
    g.batch(
        "rejected_schema_mismatch",
        "Structurally valid, but block 1 announces schema 2: the whole batch is refused (block 0 is not \
         applied either).",
        uibatch::encode_vector(0, 8, &[&a, &schema2]),
        Intended::Ok,
    );
    g.batch(
        "rejected_block_codec_error",
        "Structurally valid envelope whose block 1 is a truncated TLV block: the whole batch is refused.",
        uibatch::encode_vector(0, 9, &[&a, &truncated]),
        Intended::Ok,
    );
    g.batch(
        "seq_jump_first",
        "batch_seq 41.",
        uibatch::encode_vector(0, 41, &[&a]),
        Intended::Ok,
    );
    g.batch(
        "seq_jump_second",
        "batch_seq 43 (42 is missing).",
        uibatch::encode_vector(0, 43, &[&b]),
        Intended::Ok,
    );
    g.batch(
        "seq_wrap_first",
        "batch_seq u32::MAX.",
        uibatch::encode_vector(0, u32::MAX, &[&a]),
        Intended::Ok,
    );
    g.batch(
        "seq_wrap_second",
        "batch_seq 0 after u32::MAX (wrap-around, not a jump).",
        uibatch::encode_vector(0, 0, &[&b]),
        Intended::Ok,
    );

    // ----- structural errors -----
    let valid_one = uibatch::encode_vector(0, 1, &[&a]);
    let header_only = uibatch::encode_vector(0, 1, &[]);
    let patched = |offset: usize, bytes: &[u8]| {
        let mut v = valid_one.clone();
        v[offset..offset + bytes.len()].copy_from_slice(bytes);
        v
    };
    g.batch(
        "err_empty",
        "Zero bytes.",
        Vec::new(),
        Intended::Err("Truncated"),
    );
    g.batch(
        "err_header_truncated",
        "15 of the 16 header bytes.",
        header_only[..HEADER_BYTES - 1].to_vec(),
        Intended::Err("Truncated"),
    );
    g.batch(
        "err_bad_magic",
        "Magic \"LRUX\".",
        patched(0, b"LRUX"),
        Intended::Err("BadMagic"),
    );
    g.batch(
        "err_version_0",
        "Version 0.",
        patched(4, &0u16.to_le_bytes()),
        Intended::Err("UnsupportedVersion"),
    );
    g.batch(
        "err_version_2",
        "Version 2.",
        patched(4, &(VERSION + 1).to_le_bytes()),
        Intended::Err("UnsupportedVersion"),
    );
    g.batch(
        "err_unknown_flag_bit3",
        "Flag bit 3 (undefined) set.",
        patched(6, &(1u16 << 3).to_le_bytes()),
        Intended::Err("UnknownFlags"),
    );
    g.batch(
        "err_unknown_flag_bit15_with_reset",
        "RESET plus undefined flag bit 15.",
        patched(6, &(FLAG_RESET | (1u16 << 15)).to_le_bytes()),
        Intended::Err("UnknownFlags"),
    );
    g.batch(
        "err_reserved_nonzero",
        "Reserved field 1.",
        patched(14, &1u16.to_le_bytes()),
        Intended::Err("ReservedNonZero"),
    );
    let mut flags_and_reserved = patched(6, &(1u16 << 4).to_le_bytes());
    flags_and_reserved[14..16].copy_from_slice(&1u16.to_le_bytes());
    g.batch(
        "err_unknown_flags_before_reserved",
        "Undefined flag and non-zero reserved field: flags are checked first.",
        flags_and_reserved,
        Intended::Err("UnknownFlags"),
    );
    let mut length_cut = uibatch::encode_vector(0, 1, &[]);
    length_cut[12..14].copy_from_slice(&1u16.to_le_bytes());
    length_cut.extend_from_slice(&[7, 0]);
    g.batch(
        "err_block_length_truncated",
        "One block announced; only 2 bytes of its u32 length are present.",
        length_cut,
        Intended::Err("Truncated"),
    );
    let mut body_cut = uibatch::encode_vector(0, 1, &[]);
    body_cut[12..14].copy_from_slice(&1u16.to_le_bytes());
    body_cut.extend_from_slice(&40u32.to_le_bytes());
    body_cut.extend_from_slice(&[0; 10]);
    g.batch(
        "err_block_body_truncated",
        "A block announcing 40 bytes with 10 present.",
        body_cut,
        Intended::Err("Truncated"),
    );
    g.batch(
        "err_block_count_exceeds",
        "Two blocks announced, one present.",
        patched(12, &2u16.to_le_bytes()),
        Intended::Err("Truncated"),
    );
    let mut trailing = valid_one.clone();
    trailing.extend_from_slice(&[0, 0, 0]);
    g.batch(
        "err_trailing_bytes",
        "Three bytes after the last block.",
        trailing,
        Intended::Err("TrailingBytes"),
    );
    let mut trailing_empty = header_only;
    trailing_empty.push(0x42);
    g.batch(
        "err_trailing_bytes_no_blocks",
        "block_count 0 followed by one byte.",
        trailing_empty,
        Intended::Err("TrailingBytes"),
    );
    g.synthetic_batch(
        "err_too_large",
        "One byte over the 4 MiB limit: refused by size before any field is read.",
        (MAX_BATCH_BYTES + 1, 0),
        Intended::Err("TooLarge"),
    );
    g.synthetic_batch(
        "max_size_is_not_too_large",
        "Exactly 4 MiB of zero bytes: the size is accepted (the limit is inclusive), the magic is not.",
        (MAX_BATCH_BYTES, 0),
        Intended::Err("BadMagic"),
    );

    g.sequence(
        "in_order",
        "Consecutive batch_seq values: no gap.",
        &["single_block", "multi_block", "empty_batch"],
    );
    g.sequence(
        "seq_jump",
        "batch_seq 41 then 43: the jump is treated as GAP_BEFORE.",
        &["seq_jump_first", "seq_jump_second"],
    );
    g.sequence(
        "seq_wrap",
        "batch_seq u32::MAX then 0: wrap-around is not a jump.",
        &["seq_wrap_first", "seq_wrap_second"],
    );
    g.sequence(
        "reset_restarts_sequence",
        "batch_seq 1 then a RESET batch with batch_seq 0: a RESET starts a new sequence.",
        &["single_block", "reset"],
    );
    g.sequence(
        "gap_flag",
        "batch_seq 3 then 4 with GAP_BEFORE: a gap without a jump.",
        &["empty_batch", "gap_before"],
    );
}

/// Every golden file plus `manifest.json`.
pub fn golden_files() -> Vec<OutputFile> {
    let mut g = Golden::default();
    tlv_vectors(&mut g);
    ui_batch_vectors(&mut g);
    let manifest = Manifest {
        generated: "GENERATED - do not edit; run cargo run -p gen-ts",
        telemetry_schema_version: TELEMETRY_SCHEMA_VERSION,
        ui_batch_format: UiBatchFormat {
            magic: String::from_utf8_lossy(&MAGIC).into_owned(),
            version: VERSION,
            header_bytes: HEADER_BYTES,
            flag_reset: FLAG_RESET,
            flag_backfill: FLAG_BACKFILL,
            flag_gap_before: FLAG_GAP_BEFORE,
            max_batch_bytes: MAX_BATCH_BYTES,
        },
        notes: vec![
            "f32/f64 values are hex bit patterns; u64/i64 values are decimal strings.",
            "tlv[].expect is telemetry_model::codec::decode of the file; uiBatch[].expect.ok.apply is \
             TelemetryFrame::apply_tlv of every block, in order, on a fresh frame.",
            "TooManyRecords is an encoder-only error (record_count is a u16 on the wire); no decode vector exists.",
            "uiBatchSynthetic[] vectors are too large to commit as files: the batch is `length` bytes, every one \
             equal to `fill`; expect is the reference parser's result on exactly those bytes.",
        ],
        tlv: g.tlv,
        ui_batch: g.ui,
        ui_batch_synthetic: g.synthetic,
        ui_batch_sequences: g.sequences,
    };
    let mut json = serde_json::to_string_pretty(&manifest).expect("manifest serializes");
    json.push('\n');
    let mut files = g.files;
    files.push(OutputFile {
        path: format!("{GOLDEN_DIR}/manifest.json"),
        contents: json.into_bytes(),
    });
    files.push(OutputFile {
        path: format!("{GOLDEN_DIR}/.gitattributes"),
        contents: b"# GENERATED - do not edit; run cargo run -p gen-ts\n*.bin binary\n".to_vec(),
    });
    files
}

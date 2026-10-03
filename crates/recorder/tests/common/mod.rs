//! Helpers shared by the integration tests (each test binary uses a subset).
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use recorder::{RecordedRecord, RecordingStore, SessionId, SessionMeta, TimeRange};
use telemetry_model::codec::{RELAY_NS_UNKNOWN, TlvEncoder, TlvHeader};
use telemetry_model::{ChannelId, Sample, TELEMETRY_SCHEMA_VERSION, Value};

/// One telemetry sample every 10 ms (100 Hz).
pub const PERIOD_NS: u64 = 10_000_000;
/// Channels per telemetry record in the realistic payloads (the canonical frame's order of magnitude).
pub const CHANNELS: u16 = 150;
/// First channel id used by the synthetic payloads.
const FIRST_CHANNEL: u16 = 0x0100;

/// A unique directory under the system temp directory, removed (best effort) when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir()
            .join("lmu-recorder-it")
            .join(format!("{prefix}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn meta() -> SessionMeta {
    SessionMeta {
        app_version: "0.1.0-it".into(),
        telemetry_schema_version: TELEMETRY_SCHEMA_VERSION,
        ..Default::default()
    }
}

/// Deterministic xorshift64* generator (test data only).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in [-1, 1).
    pub fn signed_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next_u64() >> 56) as u8).collect()
    }
}

/// Appends one synthetic telemetry TLV block (telemetry-model codec, i.e. the network encoding) at
/// `capture_ns`: `channels` channels with smooth signals plus sensor-like noise (mostly f32, some integer
/// counters and flags, a few unavailable), roughly 1 KB for 150 channels.
pub fn telemetry_block(buf: &mut Vec<u8>, capture_ns: u64, channels: u16, rng: &mut Rng) {
    let t = capture_ns as f64 / 1e9;
    let header = TlvHeader {
        schema_version: TELEMETRY_SCHEMA_VERSION,
        capture_mono_ns: capture_ns,
        capture_relay_ns_est: RELAY_NS_UNKNOWN,
        source_et_s: t,
    };
    let mut enc = TlvEncoder::begin(buf, &header);
    for c in 0..channels {
        let id = ChannelId(FIRST_CHANNEL + c);
        let k = f64::from(c);
        let sample = match c % 30 {
            // Unavailable channels carry no value (never a placeholder zero).
            0 => Sample::unavailable(),
            1..=3 => Sample::valid(Value::I32((t * (k + 1.0)) as i32)),
            4..=5 => Sample::valid(Value::U32((t * 10.0) as u32 + u32::from(c))),
            6 => Sample::valid(Value::U8(((t / 30.0) as u64 % 8) as u8)),
            7 => Sample::valid(Value::Bool((t as u64 + u64::from(c)).is_multiple_of(7))),
            _ => {
                let signal = 50.0 * (t * (0.05 + k * 0.013)).sin() + 0.5 * k;
                let noise = 0.02 * rng.signed_unit();
                Sample::valid(Value::F32((signal + noise) as f32))
            }
        };
        enc.push(id, &sample).expect("valid sample");
    }
    enc.finish();
}

/// Every record of a session, in recording order (panics on a verification error).
pub fn read_all(store: &RecordingStore, id: SessionId) -> Vec<RecordedRecord> {
    store
        .open_reader(id)
        .expect("reader")
        .records(TimeRange::ALL)
        .expect("stream")
        .collect::<Result<Vec<_>, _>>()
        .expect("every chunk verifies")
}

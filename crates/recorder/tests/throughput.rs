//! Recorder benchmarks (ignored by default; numbers are meaningful in release builds only):
//!
//! ```text
//! cargo test -p recorder --release --test throughput -- --ignored --nocapture --test-threads 1
//! ```
//!
//! * `simulated_hour_at_100_hz_with_150_channels`: one hour of 100 Hz telemetry (150-channel TLV records from
//!   the shared codec) written as fast as the writer accepts it, with production settings. Reports bytes per
//!   recorded hour, compression ratio, writer CPU time and readback speed.
//! * `real_time_minute_at_100_hz`: one minute (or `RECORDER_BENCH_REAL_TIME_SECONDS`) paced at the real
//!   rate, so syncs (≈ 1 per second), wall-clock chunk sealing and per-record wake-ups happen as in
//!   production. Reports the writer's CPU share and the flush + `sync_data` wall time.

mod common;

use std::time::{Duration, Instant};

use common::{CHANNELS, PERIOD_NS, Rng, TempDir, meta, telemetry_block};
use recorder::{RecordKind, RecorderConfig, RecordingStore, SessionOptions, TimeRange};

const RATE_HZ: u64 = 100;
const MB: f64 = 1e6;

#[test]
#[ignore = "benchmark: cargo test -p recorder --release --test throughput -- --ignored --nocapture"]
fn simulated_hour_at_100_hz_with_150_channels() {
    const SECONDS: u64 = 3_600;
    let dir = TempDir::new("bench-hour");
    let store = RecordingStore::open(RecorderConfig::new(dir.path())).expect("store");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("session");
    let sender = session.sender();
    let pace_at = sender.queue_capacity() / 2;
    let mut rng = Rng::new(42);
    let mut buf = Vec::with_capacity(2_048);
    let mut payload_bytes = 0u64;
    let start = Instant::now();
    for i in 0..RATE_HZ * SECONDS {
        let capture_ns = i * PERIOD_NS;
        buf.clear();
        telemetry_block(&mut buf, capture_ns, CHANNELS, &mut rng);
        payload_bytes += buf.len() as u64;
        // Benchmark-only back-pressure: measure the writer's throughput, not the overrun policy.
        while sender.queue_len() >= pace_at {
            std::thread::yield_now();
        }
        sender
            .append(RecordKind::TelemetryTlv, capture_ns, &buf)
            .expect("no overrun while paced");
    }
    let summary = session.finish().expect("finish");
    let wall = start.elapsed();
    let s = &summary.stats;
    assert_eq!(
        (s.records_written, s.records_lost()),
        (RATE_HZ * SECONDS, 0)
    );
    let cpu = summary.writer_cpu_time.unwrap_or_default();

    let read_start = Instant::now();
    let mut read = 0u64;
    for r in store
        .open_reader(summary.session_id)
        .expect("reader")
        .records(TimeRange::ALL)
        .expect("stream")
    {
        r.expect("verified record");
        read += 1;
    }
    let read_wall = read_start.elapsed();
    assert_eq!(read, RATE_HZ * SECONDS);

    eprintln!(
        "--- recorder benchmark: 1 simulated hour, {RATE_HZ} Hz, {CHANNELS} channels per record ---"
    );
    eprintln!("records                : {}", s.records_written);
    eprintln!(
        "payload per record     : {:.0} B (TLV, telemetry-model codec)",
        payload_bytes as f64 / read as f64
    );
    eprintln!(
        "raw chunk bodies       : {:.1} MB",
        s.raw_bytes_written as f64 / MB
    );
    eprintln!(
        "on disk (MB per hour)  : {:.1} MB in {} segment(s), {} chunks",
        s.bytes_written as f64 / MB,
        summary.segments.len(),
        s.chunks_written
    );
    eprintln!(
        "compression ratio      : {:.2}x (zstd level 3)",
        s.raw_bytes_written as f64 / s.bytes_written as f64
    );
    eprintln!(
        "writer CPU time        : {:.2} s for one hour of data (data path only: framing, zstd, CRC, \
         writes, {} syncs; real-time wake-ups and per-second syncs are measured by the real-time run)",
        cpu.as_secs_f64(),
        s.syncs
    );
    eprintln!(
        "wall time              : {:.2} s ({:.0} records/s accepted)",
        wall.as_secs_f64(),
        read as f64 / wall.as_secs_f64()
    );
    eprintln!(
        "readback               : {:.2} s for the full hour ({:.0} records/s)",
        read_wall.as_secs_f64(),
        read as f64 / read_wall.as_secs_f64()
    );
}

/// Overrides the length of the real-time run (seconds). Windows charges thread CPU time per scheduler tick
/// (15.6 ms), so the CPU share of a mostly idle thread needs a long run to be measured precisely.
const REAL_TIME_SECONDS_ENV: &str = "RECORDER_BENCH_REAL_TIME_SECONDS";
const DEFAULT_REAL_TIME_SECONDS: u64 = 60;

#[test]
#[ignore = "benchmark (60 s): cargo test -p recorder --release --test throughput -- --ignored --nocapture"]
fn real_time_minute_at_100_hz() {
    let seconds: u64 = std::env::var(REAL_TIME_SECONDS_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_REAL_TIME_SECONDS);
    let dir = TempDir::new("bench-minute");
    let store = RecordingStore::open(RecorderConfig::new(dir.path())).expect("store");
    let session = store
        .start_session(meta(), SessionOptions::new())
        .expect("session");
    let mut rng = Rng::new(43);
    let mut buf = Vec::with_capacity(2_048);
    let start = Instant::now();
    for i in 0..RATE_HZ * seconds {
        let due = start + Duration::from_nanos(i * PERIOD_NS);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        buf.clear();
        telemetry_block(&mut buf, i * PERIOD_NS, CHANNELS, &mut rng);
        session
            .append(RecordKind::TelemetryTlv, i * PERIOD_NS, &buf)
            .expect("append");
    }
    let summary = session.finish().expect("finish");
    let s = &summary.stats;
    assert_eq!(
        (s.records_written, s.records_lost()),
        (RATE_HZ * seconds, 0)
    );
    let cpu = summary.writer_cpu_time.unwrap_or_default();
    let secs = seconds as f64;
    eprintln!("--- recorder benchmark: {seconds} s paced at {RATE_HZ} Hz ---");
    eprintln!("chunks                 : {}", s.chunks_written);
    eprintln!(
        "syncs                  : {} (flush + sync_data wall time: mean {:.2} ms, max {:.2} ms)",
        s.syncs,
        s.sync_time_total.as_secs_f64() * 1e3 / s.syncs.max(1) as f64,
        s.sync_time_max.as_secs_f64() * 1e3
    );
    eprintln!(
        "on disk                : {:.2} MB ({:.1} MB per hour)",
        s.bytes_written as f64 / MB,
        s.bytes_written as f64 / MB * 3_600.0 / secs
    );
    eprintln!(
        "writer CPU time        : {:.3} s ({:.3}% of one core; {:.1} s per hour; tick-sampled)",
        cpu.as_secs_f64(),
        cpu.as_secs_f64() / secs * 100.0,
        cpu.as_secs_f64() * 3_600.0 / secs
    );
}

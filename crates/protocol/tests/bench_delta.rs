//! Throughput of encoding + framing a realistic 150-channel TELEMETRY_DELTA (and decoding it again).
//!
//! Ignored by default; run with
//! `cargo test -p protocol --release --test bench_delta -- --ignored --nocapture`.

mod common;

use std::hint::black_box;
use std::time::Instant;

use bytes::BytesMut;
use protocol::frame::{Flags, Frame, FrameMeta, FramePriority, MsgType};
use protocol::{FrameDecoder, MessageError};

const ITERATIONS: u32 = 200_000;
const CHANNELS: usize = 150;

#[test]
#[ignore = "timing benchmark; run explicitly in release mode"]
fn bench_encode_and_frame_150_channel_delta() {
    let sample = common::delta_frame(1);
    let frame_len = sample.wire_len();

    // Hot path: TLV records written straight into the frame buffer (no intermediate copy).
    let start = Instant::now();
    let mut bytes_out = 0usize;
    for i in 0..ITERATIONS {
        let meta = FrameMeta::stream(
            FramePriority::P0,
            common::SESSION,
            u64::from(i) + 1,
            u64::from(i),
        );
        let frame: Frame = Frame::encode_with(
            MsgType::TelemetryDelta,
            &meta,
            Flags::empty(),
            frame_len,
            |buf| {
                common::encode_tlv(buf, CHANNELS, i);
                Ok::<(), MessageError>(())
            },
        )
        .expect("encode");
        bytes_out += frame.wire_len();
        black_box(frame);
    }
    let enc = start.elapsed();

    // Decode side: stream decoder (CRC + validation) over the same volume.
    let mut stream = BytesMut::with_capacity(frame_len * 1024);
    let start = Instant::now();
    let mut decoded = 0u32;
    let mut dec = FrameDecoder::new();
    while decoded < ITERATIONS {
        for _ in 0..1024 {
            stream.extend_from_slice(sample.as_bytes());
        }
        while let Some(f) = dec.decode_frame(&mut stream).expect("decode") {
            black_box(f);
            decoded += 1;
        }
    }
    let dec_t = start.elapsed();

    let enc_s = enc.as_secs_f64();
    let dec_s = dec_t.as_secs_f64();
    eprintln!(
        "150-channel delta: {frame_len} B/frame | encode+frame: {:.0} frames/s, {:.1} MB/s, {:.2} us/frame \
         | decode+validate: {:.0} frames/s, {:.1} MB/s",
        f64::from(ITERATIONS) / enc_s,
        bytes_out as f64 / enc_s / 1e6,
        enc_s * 1e6 / f64::from(ITERATIONS),
        f64::from(decoded) / dec_s,
        (f64::from(decoded) * frame_len as f64) / dec_s / 1e6,
    );
}

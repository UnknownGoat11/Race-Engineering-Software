//! Property tests: hostile input never panics the decoders, corruption is detected, oversized
//! lengths are refused from the header alone.

mod common;

use bytes::{Bytes, BytesMut};
use proptest::prelude::*;
use protocol::frame::{Frame, FrameError, HEADER_BYTES, MAX_FRAME_BYTES, MsgType};
use protocol::messages::Message;
use protocol::{CodecError, FrameDecoder};

/// Feeds `data` to a fresh decoder in chunks of the given sizes; returns decoded frames and the
/// first error. Panics only if the decoder itself panics.
fn feed(data: &[u8], chunks: &[usize]) -> (Vec<Frame>, Option<CodecError>) {
    let mut dec = FrameDecoder::new();
    let mut buf = BytesMut::new();
    let mut out = Vec::new();
    let mut pos = 0;
    let mut sizes = chunks.iter().cycle();
    while pos < data.len() {
        let n = (*sizes.next().unwrap_or(&1)).max(1).min(data.len() - pos);
        buf.extend_from_slice(&data[pos..pos + n]);
        pos += n;
        loop {
            match dec.decode_frame(&mut buf) {
                Ok(Some(f)) => {
                    assert!(f.wire_len() <= MAX_FRAME_BYTES);
                    // Message decoding of any validated frame must not panic either.
                    let _ = Message::from_frame(&f);
                    out.push(f);
                }
                Ok(None) => break,
                Err(e) => return (out, Some(e)),
            }
        }
    }
    (out, None)
}

fn valid_stream() -> (Vec<u8>, Vec<Frame>) {
    let frames = common::all_frames();
    let mut data = Vec::new();
    for f in &frames {
        data.extend_from_slice(f.as_bytes());
    }
    (data, frames)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_bytes_never_panic(data in proptest::collection::vec(any::<u8>(), 0..4096),
                                chunks in proptest::collection::vec(1usize..300, 1..8)) {
        let _ = feed(&data, &chunks);
    }

    #[test]
    fn random_bytes_behind_a_valid_magic_never_panic(mut data in proptest::collection::vec(any::<u8>(), 0..2048),
                                                     chunks in proptest::collection::vec(1usize..64, 1..4)) {
        // Make the magic and major valid so the fuzzer reaches the length/CRC/reserved checks.
        if data.len() >= 3 {
            data[0] = b'L';
            data[1] = b'R';
            data[2] = protocol::PROTOCOL_MAJOR;
        }
        let _ = feed(&data, &chunks);
    }

    #[test]
    fn mutated_valid_streams_never_panic_and_corruption_is_detected(
        flips in proptest::collection::vec((any::<prop::sample::Index>(), 1u8..=255), 1..6),
        chunks in proptest::collection::vec(1usize..500, 1..6),
    ) {
        let (mut data, frames) = valid_stream();
        for (idx, xor) in &flips {
            let i = idx.index(data.len());
            data[i] ^= xor;
        }
        let (out, _err) = feed(&data, &chunks);
        // Every frame that passed is byte-identical to an original frame: a mutation can never
        // produce a "valid" corrupted frame (CRC-32C over everything).
        for f in &out {
            prop_assert!(frames.iter().any(|o| o.as_bytes() == f.as_bytes()), "corrupted frame accepted");
        }
    }

    #[test]
    fn single_byte_corruption_of_any_frame_is_detected(frame_idx in any::<prop::sample::Index>(),
                                                       byte_idx in any::<prop::sample::Index>(),
                                                       xor in 1u8..=255) {
        let frames = common::all_frames();
        let f = &frames[frame_idx.index(frames.len())];
        let mut raw = f.as_bytes().to_vec();
        let i = byte_idx.index(raw.len());
        raw[i] ^= xor;
        prop_assert!(Frame::decode(Bytes::from(raw.clone())).is_err());
        let (out, _) = feed(&raw, &[raw.len()]);
        prop_assert!(out.is_empty());
    }

    #[test]
    fn random_payloads_never_panic_message_decode(t in any::<prop::sample::Index>(),
                                                  payload in proptest::collection::vec(any::<u8>(), 0..512)) {
        let msg_type = MsgType::ALL[t.index(MsgType::ALL.len())];
        if let Ok(Message::TelemetryDelta(p) | Message::TelemetrySnapshot(p) | Message::WeatherUpdate(p)) =
            Message::decode(msg_type, Bytes::from(payload))
        {
            let _ = p.decode();
        }
    }

    #[test]
    fn valid_streams_decode_identically_for_any_chunking(chunks in proptest::collection::vec(1usize..2000, 1..10)) {
        let (data, frames) = valid_stream();
        let (out, err) = feed(&data, &chunks);
        prop_assert!(err.is_none());
        prop_assert_eq!(out, frames);
    }
}

#[test]
fn length_u32_max_is_rejected_before_allocation() {
    let f = common::delta_frame(1);
    let mut header = f.as_bytes()[..HEADER_BYTES].to_vec();
    header[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut buf = BytesMut::with_capacity(HEADER_BYTES);
    buf.extend_from_slice(&header);
    let cap = buf.capacity();
    let mut dec = FrameDecoder::new();
    let err = dec.decode_frame(&mut buf).expect_err("must reject");
    assert!(
        matches!(err, CodecError::Frame(FrameError::PayloadTooLarge { len, .. }) if len == u64::from(u32::MAX))
    );
    assert_eq!(
        buf.capacity(),
        cap,
        "decoder reserved memory for an oversized frame"
    );
    assert_eq!(buf.len(), HEADER_BYTES, "nothing consumed");
}

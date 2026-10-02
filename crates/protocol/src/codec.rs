//! Streaming frame codec for TCP byte streams (docs/PROTOCOL.md §1: one `read()` ≠ one frame).
//!
//! [`FrameDecoder`] works on an accumulating `BytesMut`: it returns `Ok(None)` until a complete
//! frame is buffered, yields frames one at a time when several arrived in one read, and rejects a
//! bad magic, a foreign protocol major or an oversized `payload_len` from the first header bytes,
//! **before** anything is reserved for the frame. The decoder itself never reserves buffer space;
//! the I/O layer may use [`FrameDecoder::bytes_needed`] as a read-size hint (always bounded by
//! `MAX_FRAME_BYTES`).
//!
//! Any decode error is fatal for the connection (framing can no longer be trusted): the decoder is
//! poisoned and keeps returning [`CodecError::Poisoned`].
//!
//! Decoded frames are zero-copy views of the read buffer and keep its whole allocation alive (up to
//! about `MAX_FRAME_BYTES` after a large frame). Call [`Frame::detach`] on any frame that is stored
//! beyond the current dispatch (relay join cache, latest-wins slots, reorder buffers).

use std::io;

use bytes::{BufMut, BytesMut};
use thiserror::Error;
use tokio_util::codec::{Decoder, Encoder};

use crate::frame::{
    Frame, FrameError, HEADER_BYTES, MAX_PAYLOAD_BYTES, peek_frame_len, validate_frame,
};

/// Errors of the streaming codec.
#[derive(Debug, Error)]
pub enum CodecError {
    /// The byte stream violates the frame format. Fatal: close the connection.
    #[error("invalid frame on the stream")]
    Frame(#[from] FrameError),
    /// A previous call failed; the stream position is unknown.
    #[error("decoder is poisoned by an earlier framing error")]
    Poisoned,
    /// I/O error of the underlying transport (required by `tokio_util::codec`).
    #[error("transport I/O error")]
    Io(#[from] io::Error),
}

/// Streaming frame decoder (validation steps 1–5 of §1).
#[derive(Debug, Clone)]
pub struct FrameDecoder {
    max_payload: usize,
    poisoned: bool,
    frames_decoded: u64,
    bytes_decoded: u64,
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameDecoder {
    /// Decoder accepting payloads up to `MAX_PAYLOAD_BYTES`.
    pub const fn new() -> Self {
        Self::with_max_payload(MAX_PAYLOAD_BYTES)
    }

    /// Decoder with a tighter payload limit (e.g. the relay's announced limit). Values above
    /// `MAX_PAYLOAD_BYTES` are clamped to it.
    pub const fn with_max_payload(max_payload: usize) -> Self {
        let max_payload = if max_payload < MAX_PAYLOAD_BYTES {
            max_payload
        } else {
            MAX_PAYLOAD_BYTES
        };
        Self {
            max_payload,
            poisoned: false,
            frames_decoded: 0,
            bytes_decoded: 0,
        }
    }

    /// Effective payload limit.
    pub const fn max_payload(&self) -> usize {
        self.max_payload
    }

    /// Whether a framing error has occurred.
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Frames decoded so far.
    pub const fn frames_decoded(&self) -> u64 {
        self.frames_decoded
    }

    /// Bytes consumed by decoded frames so far.
    pub const fn bytes_decoded(&self) -> u64 {
        self.bytes_decoded
    }

    /// How many more bytes are needed to complete the frame at the start of `buf` (header first).
    /// Returns 0 if a complete frame is buffered, and an error if the buffered header is invalid.
    pub fn bytes_needed(&self, buf: &[u8]) -> Result<usize, FrameError> {
        match peek_frame_len(buf, self.max_payload)? {
            None => Ok(HEADER_BYTES - buf.len()),
            Some(len) => Ok(len.saturating_sub(buf.len())),
        }
    }

    /// Decodes the next complete frame from the front of `src`, consuming its bytes. Returns
    /// `Ok(None)` if more bytes are needed.
    pub fn decode_frame(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, CodecError> {
        if self.poisoned {
            return Err(CodecError::Poisoned);
        }
        match self.try_decode(src) {
            Ok(frame) => Ok(frame),
            Err(e) => {
                self.poisoned = true;
                tracing::warn!(component = "protocol", error = %e, "framing error, stream is untrusted");
                Err(CodecError::Frame(e))
            }
        }
    }

    fn try_decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, FrameError> {
        // Steps 1–3 on the header only; nothing is reserved before they pass.
        let Some(frame_len) = peek_frame_len(src, self.max_payload)? else {
            return Ok(None);
        };
        if src.len() < frame_len {
            return Ok(None);
        }
        let bytes = src.split_to(frame_len).freeze();
        // Steps 4–5 (re-checks 1–3 on the exact frame).
        let (header, trailer) = validate_frame(&bytes, self.max_payload)?;
        self.frames_decoded = self.frames_decoded.saturating_add(1);
        self.bytes_decoded = self.bytes_decoded.saturating_add(frame_len as u64);
        Ok(Some(Frame::from_validated(header, trailer, bytes)))
    }
}

impl Decoder for FrameDecoder {
    type Item = Frame;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, CodecError> {
        self.decode_frame(src)
    }

    fn decode_eof(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, CodecError> {
        match self.decode_frame(src)? {
            Some(frame) => Ok(Some(frame)),
            None if src.is_empty() => Ok(None),
            None => {
                self.poisoned = true;
                let needed = self.bytes_needed(src).map_or(0, |n| n + src.len());
                Err(CodecError::Frame(FrameError::Truncated {
                    needed,
                    available: src.len(),
                }))
            }
        }
    }
}

/// Frame encoder: frames are already fully encoded, so this only copies their bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameEncoder;

impl Encoder<Frame> for FrameEncoder {
    type Error = CodecError;

    fn encode(&mut self, frame: Frame, dst: &mut BytesMut) -> Result<(), CodecError> {
        dst.put_slice(frame.as_bytes());
        Ok(())
    }
}

impl Encoder<&Frame> for FrameEncoder {
    type Error = CodecError;

    fn encode(&mut self, frame: &Frame, dst: &mut BytesMut) -> Result<(), CodecError> {
        dst.put_slice(frame.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{CRC_BYTES, FrameMeta, FramePriority, MsgType, RelayTrailer};

    fn frame(seq: u64, payload: &[u8]) -> Frame {
        Frame::build(
            MsgType::TelemetryDelta,
            &FrameMeta::stream(FramePriority::P0, 7, seq, seq * 10),
            payload,
        )
        .expect("build")
    }

    #[test]
    fn partial_reads_byte_by_byte() {
        let f = frame(1, b"hello world");
        let mut dec = FrameDecoder::new();
        let mut buf = BytesMut::new();
        let raw = f.as_bytes();
        for (i, b) in raw.iter().enumerate() {
            assert_eq!(
                dec.decode(&mut buf).expect("ok"),
                None,
                "complete too early at {i}"
            );
            buf.put_u8(*b);
        }
        assert_eq!(dec.decode(&mut buf).expect("ok"), Some(f));
        assert!(buf.is_empty());
        assert_eq!(dec.frames_decoded(), 1);
    }

    #[test]
    fn multiple_frames_per_read_and_split_across_reads() {
        let frames: Vec<Frame> = (1..=5)
            .map(|s| frame(s, &vec![s as u8; s as usize * 13]))
            .collect();
        let mut stream = Vec::new();
        for f in &frames {
            stream.extend_from_slice(f.as_bytes());
        }
        // Deliver in uneven chunks.
        let mut dec = FrameDecoder::new();
        let mut buf = BytesMut::new();
        let mut out = Vec::new();
        for chunk in stream.chunks(37) {
            buf.extend_from_slice(chunk);
            while let Some(f) = dec.decode(&mut buf).expect("decode") {
                out.push(f);
            }
        }
        assert_eq!(out, frames);
        assert_eq!(dec.decode_eof(&mut buf).expect("clean eof"), None);
    }

    #[test]
    fn frames_with_trailer_decode() {
        let f = frame(3, b"abc").with_relay_trailer(&RelayTrailer {
            relay_rx_ns: 1,
            relay_tx_ns: 2,
            relay_dropped: 3,
            replayed: false,
        });
        let mut buf = BytesMut::from(f.as_bytes());
        let got = FrameDecoder::new()
            .decode(&mut buf)
            .expect("ok")
            .expect("frame");
        assert_eq!(got.relay_trailer(), f.relay_trailer());
    }

    #[test]
    fn oversized_length_is_rejected_from_the_header_alone() {
        let mut header = frame(1, b"").as_bytes()[..HEADER_BYTES].to_vec();
        header[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        let mut buf = BytesMut::from(&header[..]);
        let cap_before = buf.capacity();
        let mut dec = FrameDecoder::new();
        assert!(matches!(
            dec.decode(&mut buf),
            Err(CodecError::Frame(FrameError::PayloadTooLarge { .. }))
        ));
        assert_eq!(buf.capacity(), cap_before, "nothing reserved");
        assert!(dec.is_poisoned());
        assert!(matches!(dec.decode(&mut buf), Err(CodecError::Poisoned)));
    }

    #[test]
    fn tighter_limit_applies() {
        let f = frame(1, &[0u8; 100]);
        let mut buf = BytesMut::from(f.as_bytes());
        let mut dec = FrameDecoder::with_max_payload(99);
        assert!(matches!(
            dec.decode(&mut buf),
            Err(CodecError::Frame(FrameError::PayloadTooLarge { .. }))
        ));
        assert_eq!(
            FrameDecoder::with_max_payload(usize::MAX).max_payload(),
            MAX_PAYLOAD_BYTES
        );
    }

    #[test]
    fn crc_corruption_poisons() {
        let f = frame(1, b"payload");
        let mut raw = f.as_bytes().to_vec();
        let n = raw.len();
        raw[n - CRC_BYTES - 2] ^= 0x40;
        let mut buf = BytesMut::from(&raw[..]);
        let mut dec = FrameDecoder::new();
        assert!(matches!(
            dec.decode(&mut buf),
            Err(CodecError::Frame(FrameError::CrcMismatch { .. }))
        ));
    }

    #[test]
    fn bad_magic_detected_with_two_bytes() {
        let mut buf = BytesMut::from(&b"GE"[..]);
        assert!(matches!(
            FrameDecoder::new().decode(&mut buf),
            Err(CodecError::Frame(FrameError::BadMagic { .. }))
        ));
    }

    #[test]
    fn truncated_stream_at_eof_is_an_error() {
        let f = frame(1, b"abcdef");
        let mut buf = BytesMut::from(&f.as_bytes()[..f.wire_len() - 1]);
        let mut dec = FrameDecoder::new();
        assert!(matches!(
            dec.decode_eof(&mut buf),
            Err(CodecError::Frame(FrameError::Truncated { .. }))
        ));
    }

    #[test]
    fn bytes_needed_hint() {
        let f = frame(1, b"abcdef");
        let dec = FrameDecoder::new();
        assert_eq!(dec.bytes_needed(&[]), Ok(HEADER_BYTES));
        assert_eq!(
            dec.bytes_needed(&f.as_bytes()[..HEADER_BYTES]),
            Ok(f.wire_len() - HEADER_BYTES)
        );
        assert_eq!(dec.bytes_needed(f.as_bytes()), Ok(0));
    }

    #[test]
    fn encoder_writes_frame_bytes() {
        let f = frame(9, b"xyz");
        let mut dst = BytesMut::new();
        FrameEncoder.encode(&f, &mut dst).expect("encode");
        FrameEncoder.encode(f.clone(), &mut dst).expect("encode");
        let mut dec = FrameDecoder::new();
        assert_eq!(dec.decode(&mut dst).expect("ok"), Some(f.clone()));
        assert_eq!(dec.decode(&mut dst).expect("ok"), Some(f));
    }
}

//! In-memory chunk assembly (writer side) and chunk body decoding (scanner/reader side).

use std::time::Instant;

use zstd::bulk::{Compressor, Decompressor};

use crate::COMPONENT;
use crate::format::{
    CHUNK_HEADER_LEN, ChunkCodec, ChunkHeader, FormatError, MAX_CHUNK_RAW_BYTES, RECORD_HEADER_LEN,
    RecordKind, encode_record, validate_chunk_records,
};

/// Accumulates the records of the chunk currently being built.
#[derive(Debug, Default)]
pub(crate) struct ChunkBuilder {
    raw: Vec<u8>,
    record_count: u32,
    /// Of `record_count`, how many are [`RecordKind::RecorderNote`]s.
    note_count: u32,
    min_ns: u64,
    max_ns: u64,
    opened_at: Option<Instant>,
}

/// A chunk ready to be written: header plus encoded body, exactly as it goes to disk.
#[derive(Debug)]
pub(crate) struct SealedChunk {
    pub(crate) header: ChunkHeader,
    pub(crate) bytes: Vec<u8>,
    /// Recorder notes among `header.record_count`.
    pub(crate) note_count: u32,
}

impl SealedChunk {
    /// Producer records (everything except recorder notes).
    pub(crate) fn producer_records(&self) -> u32 {
        self.header.record_count - self.note_count
    }
}

/// What [`ChunkBuilder::discard`] dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Discarded {
    pub(crate) producer_records: u32,
    pub(crate) notes: u32,
    /// `(min, max)` capture time of everything dropped, if anything was.
    pub(crate) range: Option<(u64, u64)>,
}

impl ChunkBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.record_count == 0
    }

    #[cfg(test)]
    pub(crate) fn raw_len(&self) -> usize {
        self.raw.len()
    }

    /// Instant at which the first record of this chunk was added.
    pub(crate) fn opened_at(&self) -> Option<Instant> {
        self.opened_at
    }

    /// True when appending a record of `payload_len` bytes would push a non-empty chunk past `max_raw`.
    /// The writer seals first in that case, so a chunk only exceeds `max_raw` when a single record does.
    pub(crate) fn would_exceed(&self, payload_len: usize, max_raw: usize) -> bool {
        !self.is_empty() && self.raw.len() + RECORD_HEADER_LEN + payload_len > max_raw
    }

    /// True when adding a record at `capture_ns` would make a non-empty chunk span `max_span_ns` or more of
    /// capture time (so a chunk always covers less than `max_span_ns`). Order-independent: the span is
    /// `max - min` over all records, whatever order they arrive in.
    pub(crate) fn would_exceed_span(&self, capture_ns: u64, max_span_ns: u64) -> bool {
        !self.is_empty() && self.max_ns.max(capture_ns) - self.min_ns.min(capture_ns) >= max_span_ns
    }

    /// Appends one record. Fails only for an invalid kind, an oversized payload, or when the chunk would
    /// exceed the format's hard raw-size limit (the writer seals before that can happen).
    pub(crate) fn push(
        &mut self,
        kind: RecordKind,
        capture_ns: u64,
        bytes: &[u8],
        now: Instant,
    ) -> Result<(), FormatError> {
        let new_len = self.raw.len() + RECORD_HEADER_LEN + bytes.len();
        if new_len > MAX_CHUNK_RAW_BYTES {
            return Err(FormatError::ChunkRawTooLarge {
                len: u32::try_from(new_len).unwrap_or(u32::MAX),
            });
        }
        encode_record(&mut self.raw, kind, capture_ns, bytes)?;
        if self.record_count == 0 {
            self.min_ns = capture_ns;
            self.max_ns = capture_ns;
            self.opened_at = Some(now);
        } else {
            self.min_ns = self.min_ns.min(capture_ns);
            self.max_ns = self.max_ns.max(capture_ns);
        }
        // Cannot overflow: every record takes ≥ 13 raw bytes and raw is capped at 64 MiB.
        self.record_count += 1;
        if kind == RecordKind::RecorderNote {
            self.note_count += 1;
        }
        Ok(())
    }

    /// Drops the pending records (used when they cannot be written) and reports what was dropped.
    pub(crate) fn discard(&mut self) -> Discarded {
        let discarded = Discarded {
            producer_records: self.record_count - self.note_count,
            notes: self.note_count,
            range: (!self.is_empty()).then_some((self.min_ns, self.max_ns)),
        };
        self.reset();
        discarded
    }

    fn reset(&mut self) {
        self.raw.clear();
        self.record_count = 0;
        self.note_count = 0;
        self.min_ns = 0;
        self.max_ns = 0;
        self.opened_at = None;
    }

    /// Encodes the pending records as chunk `seq` and resets the builder. Returns `None` when empty.
    ///
    /// The body is zstd-compressed; if compression fails or does not shrink the data (tiny or
    /// incompressible chunks) the raw bytes are stored instead (`ChunkCodec::Stored`), so sealing never
    /// loses records.
    pub(crate) fn seal(
        &mut self,
        seq: u32,
        compressor: &mut Compressor<'static>,
    ) -> Option<SealedChunk> {
        if self.is_empty() {
            return None;
        }
        // `push` keeps raw ≤ MAX_CHUNK_RAW_BYTES (64 MiB), which fits in u32.
        let raw_len = self.raw.len() as u32;
        let compressed = match compressor.compress(&self.raw) {
            Ok(body) if body.len() < self.raw.len() => Some(body),
            Ok(_) => None,
            Err(error) => {
                tracing::error!(component = COMPONENT, %error, "zstd compression failed; storing chunk uncompressed");
                None
            }
        };
        let (codec, body) = match compressed {
            Some(body) => (ChunkCodec::Zstd, body),
            None => (ChunkCodec::Stored, std::mem::take(&mut self.raw)),
        };
        let header = ChunkHeader {
            // body ≤ raw for both codecs (compressed bodies are only used when smaller), so it fits in u32.
            body_len: body.len() as u32,
            record_count: self.record_count,
            chunk_seq: seq,
            min_capture_ns: self.min_ns,
            max_capture_ns: self.max_ns,
            raw_len,
            body_crc32c: crc32c::crc32c(&body),
            codec,
        };
        let mut bytes = Vec::with_capacity(CHUNK_HEADER_LEN + body.len());
        bytes.extend_from_slice(&header.encode());
        bytes.extend_from_slice(&body);
        let note_count = self.note_count;
        self.reset();
        Some(SealedChunk {
            header,
            bytes,
            note_count,
        })
    }
}

/// Verifies (length + CRC) and decodes a chunk body into `out` (cleared first), then validates the record
/// framing against the header. Returns the record count.
///
/// Memory is bounded by `header.raw_len` (≤ [`MAX_CHUNK_RAW_BYTES`]): the decompressor is given exactly
/// that much room and fails rather than growing the buffer.
pub(crate) fn decode_body(
    header: &ChunkHeader,
    body: &[u8],
    decompressor: &mut Decompressor<'static>,
    out: &mut Vec<u8>,
) -> Result<u32, FormatError> {
    header.verify_body(body)?;
    out.clear();
    match header.codec {
        ChunkCodec::Stored => out.extend_from_slice(body),
        ChunkCodec::Zstd => {
            let raw_len = header.raw_len as usize;
            out.reserve(raw_len);
            let written = decompressor.decompress_to_buffer(body, out).map_err(|e| {
                FormatError::Decompress {
                    message: e.to_string(),
                }
            })?;
            if written != raw_len {
                return Err(FormatError::RawLengthMismatch {
                    expected: header.raw_len,
                    actual: written,
                });
            }
        }
    }
    validate_chunk_records(header, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::RecordIter;

    fn compressor() -> Compressor<'static> {
        Compressor::new(3).expect("compressor")
    }

    #[test]
    fn seal_and_decode_roundtrip_compressible() {
        let now = Instant::now();
        let mut b = ChunkBuilder::new();
        let payload = vec![7u8; 4096];
        for i in 0..10u64 {
            b.push(RecordKind::TelemetryTlv, 1_000 + i, &payload, now)
                .expect("push");
        }
        b.push(RecordKind::Marker, 500, b"bookmark", now)
            .expect("push");
        let sealed = b.seal(4, &mut compressor()).expect("non-empty");
        assert!(b.is_empty());
        assert_eq!(sealed.header.codec, ChunkCodec::Zstd);
        assert_eq!(sealed.header.record_count, 11);
        assert_eq!(sealed.header.chunk_seq, 4);
        assert_eq!(
            (sealed.header.min_capture_ns, sealed.header.max_capture_ns),
            (500, 1_009)
        );
        assert_eq!(sealed.bytes.len() as u64, sealed.header.total_len());

        let hb: [u8; CHUNK_HEADER_LEN] =
            sealed.bytes[..CHUNK_HEADER_LEN].try_into().expect("header");
        let decoded = ChunkHeader::decode(&hb).expect("decode header");
        assert_eq!(decoded, sealed.header);
        let mut raw = Vec::new();
        let mut d = Decompressor::new().expect("decompressor");
        let n = decode_body(
            &decoded,
            &sealed.bytes[CHUNK_HEADER_LEN..],
            &mut d,
            &mut raw,
        )
        .expect("body");
        assert_eq!(n, 11);
        let recs: Vec<_> = RecordIter::new(&raw)
            .collect::<Result<_, _>>()
            .expect("records");
        assert_eq!(recs[0].bytes, payload.as_slice());
        assert_eq!(recs[10].kind, RecordKind::Marker);
        assert_eq!(recs[10].bytes, b"bookmark");
    }

    #[test]
    fn tiny_chunks_are_stored_uncompressed() {
        let mut b = ChunkBuilder::new();
        b.push(RecordKind::Marker, 9, b"x", Instant::now())
            .expect("push");
        let sealed = b.seal(0, &mut compressor()).expect("non-empty");
        assert_eq!(sealed.header.codec, ChunkCodec::Stored);
        assert_eq!(sealed.header.body_len, sealed.header.raw_len);
        let mut raw = Vec::new();
        let mut d = Decompressor::new().expect("decompressor");
        let n = decode_body(
            &sealed.header,
            &sealed.bytes[CHUNK_HEADER_LEN..],
            &mut d,
            &mut raw,
        )
        .expect("body");
        assert_eq!(n, 1);
    }

    #[test]
    fn corrupted_body_is_rejected() {
        let mut b = ChunkBuilder::new();
        b.push(RecordKind::TelemetryTlv, 1, &[3u8; 2048], Instant::now())
            .expect("push");
        let sealed = b.seal(0, &mut compressor()).expect("non-empty");
        let mut body = sealed.bytes[CHUNK_HEADER_LEN..].to_vec();
        body[0] ^= 0x40;
        let mut d = Decompressor::new().expect("decompressor");
        let err =
            decode_body(&sealed.header, &body, &mut d, &mut Vec::new()).expect_err("must fail");
        assert!(matches!(err, FormatError::ChunkBodyCrc { .. }));
    }

    #[test]
    fn size_policy_and_discard() {
        let now = Instant::now();
        let mut b = ChunkBuilder::new();
        assert!(
            !b.would_exceed(1_000_000, 10),
            "an empty chunk always accepts one record"
        );
        b.push(RecordKind::TelemetryTlv, 5, &[0u8; 10], now)
            .expect("push");
        assert_eq!(b.raw_len(), RECORD_HEADER_LEN + 10);
        assert!(b.would_exceed(10, 2 * (RECORD_HEADER_LEN + 10) - 1));
        assert!(!b.would_exceed(10, 2 * (RECORD_HEADER_LEN + 10)));
        b.push(RecordKind::TelemetryTlv, 3, &[0u8; 10], now)
            .expect("push");
        b.push(RecordKind::RecorderNote, 4, &[0u8; 28], now)
            .expect("push note");
        assert_eq!(b.opened_at(), Some(now));
        assert_eq!(
            b.discard(),
            Discarded {
                producer_records: 2,
                notes: 1,
                range: Some((3, 5))
            }
        );
        assert!(b.is_empty());
        assert_eq!(
            b.discard(),
            Discarded {
                producer_records: 0,
                notes: 0,
                range: None
            }
        );
        assert!(b.seal(0, &mut compressor()).is_none());
    }

    #[test]
    fn span_policy_is_order_independent() {
        let now = Instant::now();
        let mut b = ChunkBuilder::new();
        assert!(
            !b.would_exceed_span(u64::MAX, 1),
            "an empty chunk always accepts one record"
        );
        b.push(RecordKind::TelemetryTlv, 1_000, b"a", now)
            .expect("push");
        // Span limit 1000 ns: the chunk must cover strictly less than that.
        assert!(!b.would_exceed_span(1_999, 1_000));
        assert!(b.would_exceed_span(2_000, 1_000));
        assert!(
            !b.would_exceed_span(1, 1_000),
            "an earlier record widens the span from below"
        );
        assert!(b.would_exceed_span(0, 1_000));
        b.push(RecordKind::TelemetryTlv, 500, b"b", now)
            .expect("push");
        assert!(b.would_exceed_span(1_500, 1_000));
        assert!(!b.would_exceed_span(1_499, 1_000));
    }

    #[test]
    fn sealed_chunk_counts_notes_separately() {
        let now = Instant::now();
        let mut b = ChunkBuilder::new();
        b.push(RecordKind::TelemetryTlv, 1, &[1u8; 64], now)
            .expect("push");
        b.push(RecordKind::RecorderNote, 1, &[0u8; 28], now)
            .expect("push note");
        b.push(RecordKind::Marker, 2, b"m", now).expect("push");
        let sealed = b.seal(0, &mut compressor()).expect("non-empty");
        assert_eq!(
            (
                sealed.header.record_count,
                sealed.note_count,
                sealed.producer_records()
            ),
            (3, 1, 2)
        );
    }
}

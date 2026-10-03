//! Segment scanning.
//!
//! * [`validate_segment`] — full sequential validation (header CRC, body CRC, decompression, record
//!   framing) used by startup recovery and by the integrity check when a segment is closed. It stops at
//!   the first incomplete or invalid chunk and **never modifies the file**: it reports the valid length,
//!   the chunks before it and why it stopped.
//! * [`walk_chunk_headers`] — a cheap header-only walk (seeking over bodies) used by readers to complete
//!   a chunk map; bodies are verified later, when they are actually read.

use std::fs::File;
use std::io::{self, BufReader, ErrorKind, Read, Seek, SeekFrom};
use std::path::Path;

use thiserror::Error;
use zstd::bulk::Decompressor;

use crate::chunk::decode_body;
use crate::format::{
    CHUNK_HEADER_LEN, ChunkHeader, FILE_HEADER_LEN, FileHeader, FormatError, GapNote, RecordIter,
    RecordKind,
};

/// Read buffer used for sequential scans.
const SCAN_BUFFER_BYTES: usize = 1024 * 1024;
/// Block size used when draining the remainder of a file after the scan stopped.
const DRAIN_BLOCK_BYTES: usize = 64 * 1024;

/// Location of one chunk inside a segment file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkLoc {
    /// Byte offset of the chunk header from the start of the segment file.
    pub offset: u64,
    /// The decoded (CRC-checked) chunk header.
    pub header: ChunkHeader,
}

impl ChunkLoc {
    /// Offset of the first byte after this chunk.
    pub fn end(&self) -> u64 {
        self.offset + self.header.total_len()
    }
}

/// How a torn tail looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TornKind {
    /// The file ends inside a chunk header.
    IncompleteHeader,
    /// The chunk header is valid (its CRC passed) but the file ends inside the body.
    IncompleteBody,
    /// Everything from the stop offset to the end of the file is zero bytes (space allocated by the file
    /// system whose data never reached the disk, typical after a power loss).
    ZeroFilled,
    /// The final chunk is complete in length but its body CRC fails and nothing follows it: a partially
    /// persisted last write.
    InvalidFinalChunk(FormatError),
}

/// Why a validation scan stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanStop {
    /// Every byte of the file belongs to a valid chunk.
    CleanEof,
    /// The file ends with an incomplete or never-persisted write (expected after a crash). All data
    /// before `offset` is intact.
    TornTail {
        /// Byte offset where the torn chunk starts (= the valid length).
        offset: u64,
        /// What the tail looks like.
        kind: TornKind,
    },
    /// A chunk at `offset` is invalid and more (non-zero) data follows it: corruption in the middle of
    /// the file. Records after `offset` in this segment are not recoverable by the scanner.
    Corrupt {
        /// Byte offset of the invalid chunk (= the valid length).
        offset: u64,
        /// Why the chunk is invalid.
        error: FormatError,
    },
}

impl ScanStop {
    /// Offset at which the scan stopped, if it did not reach a clean end of file.
    pub fn offset(&self) -> Option<u64> {
        match self {
            ScanStop::CleanEof => None,
            ScanStop::TornTail { offset, .. } | ScanStop::Corrupt { offset, .. } => Some(*offset),
        }
    }

    /// Human-readable one-line description.
    pub fn describe(&self) -> String {
        match self {
            ScanStop::CleanEof => "clean".to_owned(),
            ScanStop::TornTail { offset, kind } => match kind {
                TornKind::IncompleteHeader => {
                    format!("torn tail at byte {offset}: file ends inside a chunk header")
                }
                TornKind::IncompleteBody => {
                    format!("torn tail at byte {offset}: file ends inside a chunk body")
                }
                TornKind::ZeroFilled => {
                    format!("torn tail at byte {offset}: zero-filled to end of file")
                }
                TornKind::InvalidFinalChunk(e) => {
                    format!("torn tail at byte {offset}: final chunk invalid ({e})")
                }
            },
            ScanStop::Corrupt { offset, error } => {
                format!("corrupt chunk at byte {offset}: {error}")
            }
        }
    }
}

/// Result of [`validate_segment`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentScan {
    /// The validated file header.
    pub header: FileHeader,
    /// Physical length of the file (all bytes, including any invalid tail).
    pub file_len: u64,
    /// Length of the valid prefix: file header plus every chunk before the stop point.
    pub valid_len: u64,
    /// Valid chunks in file order.
    pub chunks: Vec<ChunkLoc>,
    /// `chunk_end_crc32c[i]` is the CRC-32C of bytes `[0, chunks[i].end())`: lets recovery check that a
    /// prefix the writer reported as synced is still byte-identical.
    pub chunk_end_crc32c: Vec<u32>,
    /// CRC-32C of the file header alone (bytes `[0, FILE_HEADER_LEN)`).
    pub header_crc32c: u32,
    /// Records in valid chunks (all kinds, including recorder notes).
    pub record_count: u64,
    /// Of which recorder notes ([`RecordKind::RecorderNote`]).
    pub note_count: u64,
    /// Sum of `dropped_records` over the gap notes found.
    pub gap_dropped_records: u64,
    /// Smallest capture time in the valid chunks.
    pub min_capture_ns: Option<u64>,
    /// Largest capture time in the valid chunks.
    pub max_capture_ns: Option<u64>,
    /// CRC-32C of bytes `[0, valid_len)`.
    pub valid_crc32c: u32,
    /// Why the scan stopped.
    pub stop: ScanStop,
}

/// Errors that prevent a scan from producing a result at all.
#[derive(Debug, Error)]
pub enum ScanError {
    #[error("I/O error while scanning: {0}")]
    Io(#[from] io::Error),
    #[error("segment file header is invalid: {0}")]
    Header(#[source] FormatError),
    #[error("zstd decompression context could not be created: {0}")]
    Decompressor(#[source] io::Error),
}

/// Fully validates a segment file. See [`validate_segment`].
pub fn validate_segment_file(path: &Path) -> Result<SegmentScan, ScanError> {
    let file = File::open(path)?;
    validate_segment(BufReader::with_capacity(SCAN_BUFFER_BYTES, file))
}

/// Fully validates a segment read sequentially from `reader`: file header, then every chunk (header CRC,
/// body CRC, decompression, record framing, chunk sequence). Stops at the first incomplete or invalid
/// chunk and classifies the stop (see [`ScanStop`]). Never panics on any input.
///
/// Errors only for I/O failures, an unusable file header, or failure to allocate a decompressor; a damaged
/// chunk is a normal result, not an error.
pub fn validate_segment<R: Read>(mut reader: R) -> Result<SegmentScan, ScanError> {
    let mut fh = [0u8; FILE_HEADER_LEN];
    let n = read_full(&mut reader, &mut fh)?;
    let header = FileHeader::decode(&fh[..n]).map_err(ScanError::Header)?;
    let mut decompressor = Decompressor::new().map_err(ScanError::Decompressor)?;

    let header_crc32c = crc32c::crc32c(&fh);
    let mut scan = SegmentScan {
        header,
        file_len: 0,
        valid_len: FILE_HEADER_LEN as u64,
        chunks: Vec::new(),
        chunk_end_crc32c: Vec::new(),
        header_crc32c,
        record_count: 0,
        note_count: 0,
        gap_dropped_records: 0,
        min_capture_ns: None,
        max_capture_ns: None,
        valid_crc32c: header_crc32c,
        stop: ScanStop::CleanEof,
    };
    let mut offset = FILE_HEADER_LEN as u64;
    let mut body = Vec::new();
    let mut raw = Vec::new();

    let (stop, tail_bytes) = loop {
        let mut hb = [0u8; CHUNK_HEADER_LEN];
        let n = read_full(&mut reader, &mut hb)?;
        if n == 0 {
            break (ScanStop::CleanEof, 0);
        }
        if n < CHUNK_HEADER_LEN {
            // EOF inside a header: nothing follows.
            let kind = if all_zero(&hb[..n]) {
                TornKind::ZeroFilled
            } else {
                TornKind::IncompleteHeader
            };
            break (ScanStop::TornTail { offset, kind }, n as u64);
        }
        let h = match ChunkHeader::decode(&hb) {
            Ok(h) => h,
            Err(error) => {
                // The length field cannot be trusted, so the remainder of the file decides: nothing but
                // zeros after an undecodable header is the signature of a final write that was only
                // partially persisted; anything else means data after the damage point.
                let (rest, rest_zero) = drain(&mut reader)?;
                let stop = if rest_zero {
                    let kind = if all_zero(&hb) {
                        TornKind::ZeroFilled
                    } else {
                        TornKind::InvalidFinalChunk(error)
                    };
                    ScanStop::TornTail { offset, kind }
                } else {
                    ScanStop::Corrupt { offset, error }
                };
                break (stop, CHUNK_HEADER_LEN as u64 + rest);
            }
        };
        let expected_seq = u32::try_from(scan.chunks.len()).unwrap_or(u32::MAX);
        if h.chunk_seq != expected_seq {
            // A well-formed chunk (header CRC passed) that does not belong at this position.
            let (rest, _) = drain(&mut reader)?;
            let error = FormatError::ChunkSequence {
                expected: expected_seq,
                found: h.chunk_seq,
            };
            break (
                ScanStop::Corrupt { offset, error },
                CHUNK_HEADER_LEN as u64 + rest,
            );
        }
        // The header CRC passed, so body_len is what the writer wrote (≤ MAX_CHUNK_BODY_BYTES).
        body.resize(h.body_len as usize, 0);
        let n = read_full(&mut reader, &mut body)?;
        if n < body.len() {
            break (
                ScanStop::TornTail {
                    offset,
                    kind: TornKind::IncompleteBody,
                },
                (CHUNK_HEADER_LEN + n) as u64,
            );
        }
        if let Err(error) = h.verify_body(&body) {
            let (rest, rest_zero) = drain(&mut reader)?;
            // Nothing (or only zeros) after a complete chunk with a bad body CRC: the last write was only
            // partially persisted. Anything else is corruption in the middle of the file.
            let stop = if rest == 0 || rest_zero {
                ScanStop::TornTail {
                    offset,
                    kind: TornKind::InvalidFinalChunk(error),
                }
            } else {
                ScanStop::Corrupt { offset, error }
            };
            break (stop, h.total_len() + rest);
        }
        if let Err(error) = decode_body(&h, &body, &mut decompressor, &mut raw) {
            // The CRC matched, so these are exactly the bytes the writer produced: not a torn write.
            let (rest, _) = drain(&mut reader)?;
            break (ScanStop::Corrupt { offset, error }, h.total_len() + rest);
        }
        for rec in RecordIter::new(&raw).flatten() {
            if rec.kind == RecordKind::RecorderNote {
                scan.note_count += 1;
                if let Ok(note) = GapNote::decode(rec.bytes) {
                    scan.gap_dropped_records = scan
                        .gap_dropped_records
                        .saturating_add(note.dropped_records);
                }
            }
        }
        scan.valid_crc32c = crc32c::crc32c_append(scan.valid_crc32c, &hb);
        scan.valid_crc32c = crc32c::crc32c_append(scan.valid_crc32c, &body);
        scan.record_count += u64::from(h.record_count);
        scan.min_capture_ns = Some(
            scan.min_capture_ns
                .map_or(h.min_capture_ns, |m| m.min(h.min_capture_ns)),
        );
        scan.max_capture_ns = Some(
            scan.max_capture_ns
                .map_or(h.max_capture_ns, |m| m.max(h.max_capture_ns)),
        );
        scan.chunks.push(ChunkLoc { offset, header: h });
        scan.chunk_end_crc32c.push(scan.valid_crc32c);
        offset += h.total_len();
        scan.valid_len = offset;
    };
    scan.file_len = offset + tail_bytes;
    scan.stop = stop;
    Ok(scan)
}

impl SegmentScan {
    /// CRC-32C of bytes `[0, end)` when `end` is the file header end or a valid chunk boundary.
    pub fn prefix_crc32c(&self, end: u64) -> Option<u32> {
        if end == FILE_HEADER_LEN as u64 {
            return Some(self.header_crc32c);
        }
        let i = self.chunks.binary_search_by_key(&end, ChunkLoc::end).ok()?;
        self.chunk_end_crc32c.get(i).copied()
    }

    /// Records in valid chunks excluding recorder notes (i.e. producer records).
    pub fn producer_record_count(&self) -> u64 {
        self.record_count.saturating_sub(self.note_count)
    }
}

/// Why a header walk stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkStop {
    /// Reached the requested limit exactly.
    Limit,
    /// The next chunk is incomplete before the limit (normal at the tail of a segment being written).
    Incomplete {
        /// Byte offset of the incomplete chunk.
        offset: u64,
    },
    /// The next header does not decode or is out of sequence.
    Invalid {
        /// Byte offset of the invalid header.
        offset: u64,
        /// Why it is invalid.
        error: FormatError,
    },
}

/// Result of [`walk_chunk_headers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderWalk {
    /// Chunks found, in file order.
    pub chunks: Vec<ChunkLoc>,
    /// Offset just after the last chunk found.
    pub end: u64,
    /// Why the walk stopped.
    pub stop: WalkStop,
}

/// Walks chunk headers from `offset` (a chunk boundary whose chunk has sequence `seq`) up to `limit`
/// bytes, seeking over bodies. Headers are fully checked (magic, header CRC, sequence); bodies are not
/// read, so their CRC is verified only when a reader decodes them.
pub fn walk_chunk_headers<R: Read + Seek>(
    reader: &mut R,
    offset: u64,
    seq: u32,
    limit: u64,
) -> io::Result<HeaderWalk> {
    let mut chunks = Vec::new();
    let mut pos = offset;
    let mut next_seq = seq;
    let stop = loop {
        if pos >= limit {
            break WalkStop::Limit;
        }
        if limit - pos < CHUNK_HEADER_LEN as u64 {
            break WalkStop::Incomplete { offset: pos };
        }
        reader.seek(SeekFrom::Start(pos))?;
        let mut hb = [0u8; CHUNK_HEADER_LEN];
        if read_full(reader, &mut hb)? < CHUNK_HEADER_LEN {
            break WalkStop::Incomplete { offset: pos };
        }
        let h = match ChunkHeader::decode(&hb) {
            Ok(h) => h,
            Err(error) => break WalkStop::Invalid { offset: pos, error },
        };
        if h.chunk_seq != next_seq {
            let error = FormatError::ChunkSequence {
                expected: next_seq,
                found: h.chunk_seq,
            };
            break WalkStop::Invalid { offset: pos, error };
        }
        let loc = ChunkLoc {
            offset: pos,
            header: h,
        };
        if loc.end() > limit {
            break WalkStop::Incomplete { offset: pos };
        }
        chunks.push(loc);
        pos = loc.end();
        next_seq = next_seq.saturating_add(1);
    };
    Ok(HeaderWalk {
        chunks,
        end: pos,
        stop,
    })
}

/// Reads until `buf` is full or EOF. Returns the number of bytes read.
pub(crate) fn read_full<R: Read + ?Sized>(reader: &mut R, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// Consumes the rest of the stream. Returns the number of bytes and whether they were all zero.
fn drain<R: Read>(reader: &mut R) -> io::Result<(u64, bool)> {
    let mut block = vec![0u8; DRAIN_BLOCK_BYTES];
    let mut total = 0u64;
    let mut zero = true;
    loop {
        let n = read_full(reader, &mut block)?;
        if n == 0 {
            return Ok((total, zero));
        }
        zero &= all_zero(&block[..n]);
        total += n as u64;
    }
}

fn all_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| b == 0)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::time::Instant;

    use uuid::Uuid;
    use zstd::bulk::Compressor;

    use super::*;
    use crate::chunk::ChunkBuilder;
    use crate::format::FORMAT_VERSION;

    /// Builds a segment with `chunks` chunks of `per_chunk` records each and returns the bytes plus the
    /// offset of every chunk.
    fn build_segment(chunks: u32, per_chunk: u64) -> (Vec<u8>, Vec<u64>) {
        let header = FileHeader {
            format_version: FORMAT_VERSION,
            telemetry_schema_version: 1,
            session_id: Uuid::from_u128(7),
            segment_index: 0,
            created_unix_ns: 1,
            created_mono_ns: 2,
            app_version: "test".into(),
        };
        let mut out = header.encode().expect("header").to_vec();
        let mut offsets = Vec::new();
        let mut c = Compressor::new(1).expect("compressor");
        let now = Instant::now();
        for seq in 0..chunks {
            let mut b = ChunkBuilder::new();
            for i in 0..per_chunk {
                let ns = u64::from(seq) * 1_000 + i;
                let payload: Vec<u8> = (0..64u8).map(|x| x.wrapping_mul(i as u8)).collect();
                b.push(RecordKind::TelemetryTlv, ns, &payload, now)
                    .expect("push");
            }
            let sealed = b.seal(seq, &mut c).expect("sealed");
            offsets.push(out.len() as u64);
            out.extend_from_slice(&sealed.bytes);
        }
        (out, offsets)
    }

    #[test]
    fn clean_segment_scans_to_eof() {
        let (bytes, offsets) = build_segment(4, 10);
        let scan = validate_segment(Cursor::new(&bytes)).expect("scan");
        assert_eq!(scan.stop, ScanStop::CleanEof);
        assert_eq!(scan.valid_len, bytes.len() as u64);
        assert_eq!(scan.file_len, bytes.len() as u64);
        assert_eq!(
            scan.chunks.iter().map(|c| c.offset).collect::<Vec<_>>(),
            offsets
        );
        assert_eq!(scan.record_count, 40);
        assert_eq!(scan.valid_crc32c, crc32c::crc32c(&bytes));
        assert_eq!(
            (scan.min_capture_ns, scan.max_capture_ns),
            (Some(0), Some(3_009))
        );
    }

    #[test]
    fn truncation_at_every_offset_of_last_chunk_keeps_complete_chunks() {
        let (bytes, offsets) = build_segment(5, 7);
        let last = *offsets.last().expect("chunks");
        for cut in last..bytes.len() as u64 {
            let scan =
                validate_segment(Cursor::new(&bytes[..cut as usize])).expect("scan must not fail");
            assert_eq!(scan.chunks.len(), 4, "cut at {cut}");
            assert_eq!(scan.valid_len, last, "cut at {cut}");
            assert_eq!(scan.file_len, cut);
            assert_eq!(scan.record_count, 28);
            assert_eq!(scan.valid_crc32c, crc32c::crc32c(&bytes[..last as usize]));
            if cut == last {
                assert_eq!(scan.stop, ScanStop::CleanEof);
            } else {
                assert!(
                    matches!(scan.stop, ScanStop::TornTail { offset, .. } if offset == last),
                    "cut at {cut}: {:?}",
                    scan.stop
                );
            }
        }
    }

    #[test]
    fn zero_filled_tail_is_torn_not_corrupt() {
        let (mut bytes, _) = build_segment(2, 3);
        let valid = bytes.len() as u64;
        bytes.extend_from_slice(&[0u8; 5000]);
        let scan = validate_segment(Cursor::new(&bytes)).expect("scan");
        assert_eq!(
            scan.stop,
            ScanStop::TornTail {
                offset: valid,
                kind: TornKind::ZeroFilled
            }
        );
        assert_eq!(scan.valid_len, valid);
        assert_eq!(scan.file_len, bytes.len() as u64);
    }

    #[test]
    fn corrupt_middle_chunk_stops_there() {
        let (mut bytes, offsets) = build_segment(5, 7);
        let victim = offsets[2] as usize + CHUNK_HEADER_LEN + 3;
        bytes[victim] ^= 0xff;
        let scan = validate_segment(Cursor::new(&bytes)).expect("scan");
        assert_eq!(scan.chunks.len(), 2);
        assert_eq!(scan.valid_len, offsets[2]);
        assert!(
            matches!(&scan.stop, ScanStop::Corrupt { offset, error: FormatError::ChunkBodyCrc { .. } } if *offset == offsets[2])
        );
        // A corrupted header (length field) is caught by the header CRC.
        let (mut bytes, offsets) = build_segment(5, 7);
        bytes[offsets[1] as usize + 5] ^= 0x01;
        let scan = validate_segment(Cursor::new(&bytes)).expect("scan");
        assert!(
            matches!(&scan.stop, ScanStop::Corrupt { offset, error: FormatError::ChunkHeaderCrc { .. } } if *offset == offsets[1])
        );
    }

    #[test]
    fn bad_final_body_is_torn_tail() {
        let (mut bytes, offsets) = build_segment(3, 7);
        let n = bytes.len();
        bytes[n - 1] ^= 0x10;
        let scan = validate_segment(Cursor::new(&bytes)).expect("scan");
        assert!(
            matches!(&scan.stop, ScanStop::TornTail { offset, kind: TornKind::InvalidFinalChunk(_) } if *offset == offsets[2])
        );
    }

    #[test]
    fn short_or_foreign_header_is_an_error() {
        let (bytes, _) = build_segment(1, 1);
        assert!(matches!(
            validate_segment(Cursor::new(&bytes[..50])),
            Err(ScanError::Header(FormatError::HeaderTruncated {
                available: 50
            }))
        ));
        assert!(matches!(
            validate_segment(Cursor::new(b"not a recording at all, definitely not")),
            Err(ScanError::Header(_))
        ));
    }

    #[test]
    fn header_walk_finds_chunks_and_stops_at_incomplete_tail() {
        let (bytes, offsets) = build_segment(4, 5);
        let mut cur = Cursor::new(&bytes);
        let walk = walk_chunk_headers(&mut cur, FILE_HEADER_LEN as u64, 0, bytes.len() as u64)
            .expect("walk");
        assert_eq!(walk.stop, WalkStop::Limit);
        assert_eq!(walk.chunks.len(), 4);
        // Resume from the second chunk with a limit inside the last chunk.
        let limit = bytes.len() as u64 - 1;
        let walk = walk_chunk_headers(&mut cur, offsets[1], 1, limit).expect("walk");
        assert_eq!(walk.chunks.len(), 2);
        assert_eq!(walk.stop, WalkStop::Incomplete { offset: offsets[3] });
        // Wrong starting sequence is reported, not silently accepted.
        let walk = walk_chunk_headers(&mut cur, offsets[1], 0, limit).expect("walk");
        assert!(matches!(
            walk.stop,
            WalkStop::Invalid {
                error: FormatError::ChunkSequence { .. },
                ..
            }
        ));
    }
}

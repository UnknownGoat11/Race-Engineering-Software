//! Reading recordings back: records by capture-time range, streamed chunk by chunk.
//!
//! Memory is bounded by one chunk (its encoded body and its decoded records) plus the chunk map of the
//! segment being read, whatever the length of the session. Chunks are located through the chunk offset
//! index (one row per chunk, ≈ one per second); the part of a segment the index does not cover (a segment
//! still being written, or rows lost to index failures) is located by walking CRC-checked chunk headers.
//! Every chunk is verified (header CRC, match with its index row, body CRC, decompressed length, record
//! framing) before any of its records is returned. A damaged chunk is reported as an error item and reading
//! continues with the next chunk, so intact data after a damaged region is still returned.

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};

use zstd::bulk::Decompressor;

use crate::chunk::decode_body;
use crate::error::RecorderError;
use crate::format::{
    CHUNK_HEADER_LEN, ChunkHeader, FILE_HEADER_LEN, FileHeader, FormatError, GapNote, RecordKind,
    record_at,
};
use crate::index::{ChunkEntry, SegmentInfo, SessionInfo};
use crate::scan::{WalkStop, read_full, walk_chunk_headers};
use crate::types::{SegmentStatus, SessionId, TimeRange};

/// A segment and its indexed chunks, as known when the reader was opened.
#[derive(Debug, Clone)]
pub(crate) struct SegmentPlan {
    pub(crate) info: SegmentInfo,
    /// Indexed chunks: a contiguous prefix of the segment's chunks, in sequence order.
    pub(crate) chunks: Vec<ChunkEntry>,
}

impl SegmentPlan {
    /// Whether the segment can hold records inside `range`. Segments whose extent is not fully known (still
    /// open, or not fully indexed) are always read.
    fn may_overlap(&self, range: &TimeRange) -> bool {
        if self.info.status == SegmentStatus::Open || self.needs_walk() {
            return true;
        }
        let (mut lo, mut hi) = (self.info.first_capture_ns, self.info.last_capture_ns);
        for c in &self.chunks {
            lo = Some(lo.map_or(c.min_capture_ns, |v| v.min(c.min_capture_ns)));
            hi = Some(hi.map_or(c.max_capture_ns, |v| v.max(c.max_capture_ns)));
        }
        match (lo, hi) {
            (Some(lo), Some(hi)) => range.overlaps(lo, hi),
            // Nothing known to be in the segment.
            _ => false,
        }
    }

    /// Where a header walk must start after the indexed chunks: `(offset, chunk_seq)`.
    fn walk_start(&self) -> (u64, u32) {
        self.chunks.last().map_or((FILE_HEADER_LEN as u64, 0), |c| {
            (c.end(), c.chunk_seq.saturating_add(1))
        })
    }

    /// Whether chunks beyond the indexed ones may exist.
    fn needs_walk(&self) -> bool {
        self.info.status == SegmentStatus::Open || self.walk_start().0 < self.info.byte_size
    }
}

/// One record read back from a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRecord {
    /// What the payload contains (which decoder to use).
    pub kind: RecordKind,
    /// Capture time the producer supplied.
    pub capture_ns: u64,
    /// The payload exactly as appended.
    pub bytes: Vec<u8>,
    /// Segment the record was read from.
    pub segment_index: u32,
    /// Chunk (within the segment) the record was read from.
    pub chunk_seq: u32,
}

impl RecordedRecord {
    /// The gap note carried by a [`RecordKind::RecorderNote`] record; `None` for any other kind.
    pub fn gap_note(&self) -> Option<Result<GapNote, FormatError>> {
        (self.kind == RecordKind::RecorderNote).then(|| GapNote::decode(&self.bytes))
    }
}

/// A recorded session opened for reading (see [`crate::RecordingStore::open_reader`]). Holds only the
/// session's index rows; records are read by [`RecordStream`]s.
#[derive(Debug, Clone)]
pub struct SessionReader {
    root: PathBuf,
    session: SessionInfo,
    segments: Vec<SegmentPlan>,
}

impl SessionReader {
    pub(crate) fn new(root: PathBuf, session: SessionInfo, segments: Vec<SegmentPlan>) -> Self {
        Self {
            root,
            session,
            segments,
        }
    }

    /// The session row as it was when the reader was opened.
    pub fn session(&self) -> &SessionInfo {
        &self.session
    }

    /// The session's segments in order.
    pub fn segments(&self) -> impl Iterator<Item = &SegmentInfo> {
        self.segments.iter().map(|p| &p.info)
    }

    /// Capture-time interval covered by the session's valid data, if any (for a [`crate::ReplayClock`]).
    pub fn capture_bounds(&self) -> Option<TimeRange> {
        TimeRange::new(
            self.session.first_capture_ns?,
            self.session.last_capture_ns?,
        )
    }

    /// Streams the records whose capture time lies in `range`, in recording order (segment by segment, chunk
    /// by chunk). Chunks and segments outside the range are skipped without being read, so a range starting
    /// late in a session is a chunk-level seek.
    ///
    /// Recording order follows the order of `append` calls: capture times are non-decreasing when producers
    /// append in capture order, except for recorder notes, which carry the capture time of the first record
    /// of the gap they describe. A gap note is returned whenever its gap overlaps `range`, even when its own
    /// capture time lies before the range start, so a read starting inside a gap is never silently short.
    pub fn records(&self, range: TimeRange) -> Result<RecordStream, RecorderError> {
        let decompressor = Decompressor::new().map_err(RecorderError::Codec)?;
        Ok(RecordStream {
            root: self.root.clone(),
            session_id: self.session.id,
            range,
            pending: self.segments.clone().into_iter(),
            current: None,
            decompressor,
            body: Vec::new(),
            raw: Vec::new(),
            pos: 0,
            chunk: None,
        })
    }
}

/// Streaming iterator over recorded records (see [`SessionReader::records`]).
///
/// Yields `Err` for a segment that cannot be opened (the rest of that segment is skipped) and for a chunk
/// that fails verification (reading continues with the next chunk). Records are never returned from a
/// chunk that failed verification.
pub struct RecordStream {
    root: PathBuf,
    session_id: SessionId,
    range: TimeRange,
    pending: std::vec::IntoIter<SegmentPlan>,
    current: Option<SegmentCursor>,
    decompressor: Decompressor<'static>,
    /// Encoded body of the current chunk (reused).
    body: Vec<u8>,
    /// Decoded records of the current chunk (reused).
    raw: Vec<u8>,
    /// Next record offset in `raw`.
    pos: usize,
    /// `(segment_index, chunk_seq)` of the decoded chunk in `raw`, if any.
    chunk: Option<(u32, u32)>,
}

impl std::fmt::Debug for RecordStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordStream")
            .field("session_id", &self.session_id)
            .field("range", &self.range)
            .field("chunk", &self.chunk)
            .finish_non_exhaustive()
    }
}

impl Iterator for RecordStream {
    type Item = Result<RecordedRecord, RecorderError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // 1. Remaining records of the decoded chunk.
            if let Some((segment_index, chunk_seq)) = self.chunk {
                while self.pos < self.raw.len() {
                    match record_at(&self.raw, self.pos) {
                        Ok((rec, next)) => {
                            self.pos = next;
                            if in_range(&self.range, rec.kind, rec.capture_ns, rec.bytes) {
                                return Some(Ok(RecordedRecord {
                                    kind: rec.kind,
                                    capture_ns: rec.capture_ns,
                                    bytes: rec.bytes.to_vec(),
                                    segment_index,
                                    chunk_seq,
                                }));
                            }
                        }
                        Err(source) => {
                            // Unreachable: the framing was validated when the chunk was decoded.
                            self.pos = self.raw.len();
                            let path = self
                                .current
                                .as_ref()
                                .map(|c| c.path.clone())
                                .unwrap_or_default();
                            return Some(Err(RecorderError::Format {
                                path,
                                offset: 0,
                                source,
                            }));
                        }
                    }
                }
                self.chunk = None;
            }
            // 2. Next chunk of the current segment.
            if let Some(cur) = self.current.as_mut() {
                match cur.next_entry(&self.range) {
                    Some(Ok(entry)) => {
                        match cur.load(
                            &entry,
                            &mut self.decompressor,
                            &mut self.body,
                            &mut self.raw,
                        ) {
                            Ok(()) => {
                                self.pos = 0;
                                self.chunk = Some((cur.segment_index, entry.chunk_seq));
                            }
                            // The next call continues with the following chunk.
                            Err(e) => return Some(Err(e)),
                        }
                    }
                    Some(Err(e)) => return Some(Err(e)),
                    None => self.current = None,
                }
                continue;
            }
            // 3. Next segment.
            let plan = self.pending.next()?;
            if !plan.may_overlap(&self.range) {
                continue;
            }
            match SegmentCursor::open(&self.root, self.session_id, plan) {
                Ok(cursor) => self.current = Some(cursor),
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

/// Reading position inside one segment file.
struct SegmentCursor {
    segment_index: u32,
    path: PathBuf,
    file: File,
    /// Chunks still to visit, in file order.
    chunks: std::vec::IntoIter<ChunkEntry>,
    /// Header walk still to do once `chunks` is exhausted: `(offset, chunk_seq)`.
    walk_from: Option<(u64, u32)>,
    /// End of the readable data for the walk.
    limit: u64,
    /// Error found by the walk, reported after the chunks it found.
    deferred: Option<RecorderError>,
}

impl SegmentCursor {
    fn open(root: &Path, session_id: SessionId, plan: SegmentPlan) -> Result<Self, RecorderError> {
        let path = root.join(&plan.info.path);
        let mut file = File::open(&path)
            .map_err(|e| RecorderError::io("opening a segment for reading", &path, e))?;
        let mut hb = [0u8; FILE_HEADER_LEN];
        let n = read_full(&mut file, &mut hb)
            .map_err(|e| RecorderError::io("reading a segment header", &path, e))?;
        let header = FileHeader::decode(&hb[..n]).map_err(|source| RecorderError::Format {
            path: path.clone(),
            offset: 0,
            source,
        })?;
        if header.session_id != session_id.0 || header.segment_index != plan.info.segment_index {
            return Err(RecorderError::SegmentMismatch {
                path,
                session: session_id,
                segment: plan.info.segment_index,
                found_session: SessionId(header.session_id),
                found_segment: header.segment_index,
            });
        }
        let limit = if plan.info.status == SegmentStatus::Open {
            // Still being written: everything handed to the OS so far is readable.
            file.metadata()
                .map_err(|e| RecorderError::io("reading segment metadata", &path, e))?
                .len()
        } else {
            plan.info.byte_size
        };
        let walk_from = plan.needs_walk().then(|| plan.walk_start());
        Ok(Self {
            segment_index: plan.info.segment_index,
            path,
            file,
            chunks: plan.chunks.into_iter(),
            walk_from,
            limit,
            deferred: None,
        })
    }

    /// Next chunk overlapping `range`, walking chunk headers once the indexed chunks are exhausted.
    fn next_entry(&mut self, range: &TimeRange) -> Option<Result<ChunkEntry, RecorderError>> {
        loop {
            if let Some(entry) = self.chunks.next() {
                if range.overlaps(entry.min_capture_ns, entry.max_capture_ns) {
                    return Some(Ok(entry));
                }
                continue;
            }
            if let Some(e) = self.deferred.take() {
                return Some(Err(e));
            }
            let (offset, seq) = self.walk_from.take()?;
            match walk_chunk_headers(&mut self.file, offset, seq, self.limit) {
                Ok(walk) => {
                    if let WalkStop::Invalid { offset, error } = walk.stop {
                        self.deferred = Some(RecorderError::Format {
                            path: self.path.clone(),
                            offset,
                            source: error,
                        });
                    }
                    // `Incomplete` is the normal end of a segment that is still being written.
                    self.chunks = walk
                        .chunks
                        .iter()
                        .map(ChunkEntry::from)
                        .collect::<Vec<_>>()
                        .into_iter();
                }
                Err(e) => {
                    return Some(Err(RecorderError::io(
                        "walking chunk headers",
                        &self.path,
                        e,
                    )));
                }
            }
        }
    }

    /// Reads, verifies and decodes the chunk `entry` into `raw`.
    fn load(
        &mut self,
        entry: &ChunkEntry,
        decompressor: &mut Decompressor<'static>,
        body: &mut Vec<u8>,
        raw: &mut Vec<u8>,
    ) -> Result<(), RecorderError> {
        let offset = entry.offset;
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(io_err("seeking to a chunk", &self.path))?;
        let mut hb = [0u8; CHUNK_HEADER_LEN];
        let n = read_full(&mut self.file, &mut hb)
            .map_err(io_err("reading a chunk header", &self.path))?;
        let mismatch = |detail: String| RecorderError::IndexMismatch {
            path: self.path.clone(),
            offset,
            chunk_seq: entry.chunk_seq,
            detail,
        };
        if n < CHUNK_HEADER_LEN {
            return Err(mismatch(format!(
                "the file ends {n} bytes into the chunk header"
            )));
        }
        let h = ChunkHeader::decode(&hb).map_err(|source| RecorderError::Format {
            path: self.path.clone(),
            offset,
            source,
        })?;
        if h.chunk_seq != entry.chunk_seq
            || h.body_len != entry.body_len
            || h.record_count != entry.record_count
            || h.min_capture_ns != entry.min_capture_ns
            || h.max_capture_ns != entry.max_capture_ns
        {
            return Err(mismatch(format!(
                "file has chunk seq {} (body {} bytes, {} records, capture ns [{}, {}]), index has seq {} (body {} bytes, {} records, capture ns [{}, {}])",
                h.chunk_seq,
                h.body_len,
                h.record_count,
                h.min_capture_ns,
                h.max_capture_ns,
                entry.chunk_seq,
                entry.body_len,
                entry.record_count,
                entry.min_capture_ns,
                entry.max_capture_ns
            )));
        }
        body.resize(h.body_len as usize, 0);
        let n =
            read_full(&mut self.file, body).map_err(io_err("reading a chunk body", &self.path))?;
        if n < body.len() {
            return Err(RecorderError::Format {
                path: self.path.clone(),
                offset,
                source: FormatError::BodyLengthMismatch {
                    expected: h.body_len,
                    actual: n,
                },
            });
        }
        decode_body(&h, body, decompressor, raw).map_err(|source| RecorderError::Format {
            path: self.path.clone(),
            offset,
            source,
        })?;
        Ok(())
    }
}

/// Whether a record belongs to a read of `range`: its capture time lies in the range or, for a gap note,
/// the gap it describes `[first_capture_ns, last_capture_ns]` overlaps the range (so a read starting inside
/// a gap still reports it; the writer widens the note's chunk header to the end of the gap accordingly).
fn in_range(range: &TimeRange, kind: RecordKind, capture_ns: u64, bytes: &[u8]) -> bool {
    if range.contains(capture_ns) {
        return true;
    }
    if kind != RecordKind::RecorderNote {
        return false;
    }
    match GapNote::decode(bytes) {
        Ok(GapNote {
            first_capture_ns: Some(first),
            last_capture_ns: Some(last),
            ..
        }) if first <= last => range.overlaps(first, last),
        _ => false,
    }
}

/// `map_err` adapter attaching the operation and path to an I/O error.
fn io_err<'a>(
    op: &'static str,
    path: &'a Path,
) -> impl FnOnce(std::io::Error) -> RecorderError + 'a {
    move |e| RecorderError::io(op, path, e)
}

//! `.lmuraw` capture files: the raw byte blocks of copied frames plus their timestamps, written by
//! `lmu-diag capture` and replayed by offline normalizer tests.
//!
//! **Privacy: a capture contains driver names, the player name and server names (PII).** It never
//! contains `data.paths` unless the capturing reader was configured to copy them.
//!
//! Format (all integers little-endian; every length is checked before anything is allocated):
//!
//! ```text
//! file   := header frame*
//! header := magic "LMURAW\r\n" (8 bytes)
//!           format_version     u16 (= 2)
//!           layout_size        u32 (sizeof(SharedMemoryLayout) the offsets refer to, 324824)
//!           layout_fingerprint u64 (offsets::LAYOUT_FINGERPRINT: every struct, field, offset, size)
//!           meta_len           u32, meta: meta_len bytes of UTF-8 text ("key: value" lines)
//! frame  := record_len u32 (bytes that follow, at most 1 MiB)
//!           seq u64, capture_mono_ns u64, connection_id u64,
//!           flags u8 (bit 0: copied under the SDK lock, bit 1: frame-gate sync, else polling),
//!           block_count u8, block*
//! block  := kind u8 (1 generic, 2 scoring header, 3 vehicle rows, 4 telemetry header,
//!           5 telemetry slots 0..min(activeVehicles,104), 6 paths, 7 scoring stream)
//!           offset u32 (absolute offset in LMU_Data), len u32, len bytes
//! ```
//!
//! A replay validates every block against the layout ([`RawBlocks`] placement and count rules) and
//! runs the same [`FrameProcessor`] as the live reader (reset whenever `connection_id` changes, as
//! the live reader is on every attach), so it yields the same snapshots apart from `source`, which
//! says [`Consistency::Replayed`] and keeps whether the frame was copied under the lock. Version-1
//! files (player slot only, no fingerprint) are refused.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use thiserror::Error;

use crate::blocks::{BlockError, BlockKind, RawBlocks};
use crate::bytes::DecodeError;
use crate::compat::CompatibilityReport;
use crate::offsets::{LAYOUT_FINGERPRINT, layout};
use crate::processor::{FrameMeta, FrameOutcome, FrameProcessor};
use crate::snapshot::RawSnapshot;
use crate::state::{Consistency, FrameSource, SyncMode};
use crate::stats::ReaderStats;

/// File magic (the CR LF pair detects text-mode mangling).
pub const MAGIC: [u8; 8] = *b"LMURAW\r\n";
/// Format version written and read by this build.
pub const FORMAT_VERSION: u16 = 2;
/// Frame flag: copied while holding the SDK lock.
pub const FLAG_LOCKED: u8 = 0b01;
/// Frame flag: frame-gate synchronisation (otherwise polling).
pub const FLAG_FRAME_GATE: u8 = 0b10;
/// Largest accepted frame record (a full frame is about 130 KiB).
pub const MAX_RECORD_LEN: usize = 1 << 20;
/// Largest accepted header text.
pub const MAX_META_LEN: usize = 1 << 16;

/// Size of the fixed part of a frame record (seq, capture time, connection id, flags, block count).
const FRAME_FIXED_LEN: usize = 8 + 8 + 8 + 1 + 1;
/// Size of a block header (kind, offset, len).
const BLOCK_HEADER_LEN: usize = 1 + 4 + 4;

/// Errors reading or writing captures.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// File I/O failed.
    #[error("capture I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The file does not start with [`MAGIC`].
    #[error("not an .lmuraw capture (bad magic)")]
    BadMagic,
    /// Unknown format version.
    #[error("unsupported .lmuraw format version {0} (this build reads version {FORMAT_VERSION})")]
    Version(u16),
    /// The capture was recorded against a different `SharedMemoryLayout` size; decoding it with
    /// these offsets would be guessing.
    #[error(
        "capture was recorded for a {found}-byte LMU layout; this build decodes {expected} bytes"
    )]
    LayoutMismatch {
        /// Layout size stored in the file.
        found: u32,
        /// `sizeof(SharedMemoryLayout)` of this build.
        expected: u32,
    },
    /// The capture was recorded against a layout whose fields differ from this build's (same size,
    /// moved or retyped fields); decoding it with these offsets would be guessing.
    #[error(
        "capture was recorded for layout fingerprint {found:#018x}; this build decodes {expected:#018x} \
         (the probed offsets changed)"
    )]
    FingerprintMismatch {
        /// Fingerprint stored in the file.
        found: u64,
        /// [`LAYOUT_FINGERPRINT`] of this build.
        expected: u64,
    },
    /// A length or field is out of range.
    #[error("corrupt capture at frame {frame}: {reason}")]
    Corrupt {
        /// Zero-based frame index.
        frame: u64,
        /// What is wrong.
        reason: &'static str,
    },
    /// The file ends inside a record (the recording was cut short).
    #[error("capture is truncated inside frame {frame}")]
    Truncated {
        /// Zero-based index of the incomplete frame.
        frame: u64,
    },
    /// A block does not fit the layout.
    #[error("capture frame {frame}: {source}")]
    Block {
        /// Zero-based frame index.
        frame: u64,
        /// Placement or consistency problem.
        #[source]
        source: BlockError,
    },
    /// A frame could not be decoded.
    #[error("capture frame {frame} could not be decoded: {source}")]
    Decode {
        /// Zero-based frame index.
        frame: u64,
        /// Decoder error.
        #[source]
        source: DecodeError,
    },
    /// A frame fails the structural compatibility checks.
    #[error("capture frame {frame} fails the structural compatibility checks: {message}")]
    Incompatible {
        /// Zero-based frame index.
        frame: u64,
        /// Detected vs expected values.
        message: String,
    },
}

fn layout_size_u32() -> u32 {
    u32::try_from(layout::SIZE).unwrap_or(u32::MAX)
}

/// The capture flags describing a frame source.
pub fn flags_for(source: &FrameSource) -> u8 {
    let mut flags = 0;
    if matches!(
        source.consistency,
        Consistency::Locked | Consistency::Replayed { locked: true }
    ) {
        flags |= FLAG_LOCKED;
    }
    if source.sync == Some(SyncMode::FrameGate) {
        flags |= FLAG_FRAME_GATE;
    }
    flags
}

/// Per-frame header of a capture record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// Snapshot sequence number at capture time.
    pub seq: u64,
    /// Capture time (the capturing process's [`crate::MonoClock`]).
    pub capture_mono_ns: u64,
    /// Connection generation of the capturing reader (a change means LMU was re-attached).
    pub connection_id: u64,
    /// [`FLAG_LOCKED`] / [`FLAG_FRAME_GATE`] (see [`flags_for`]).
    pub flags: u8,
}

impl FrameInfo {
    /// The header of a snapshot's record.
    pub fn of(snapshot: &RawSnapshot) -> Self {
        Self {
            seq: snapshot.seq,
            capture_mono_ns: snapshot.capture_mono_ns,
            connection_id: snapshot.connection_id,
            flags: flags_for(&snapshot.source),
        }
    }
}

/// Writes `.lmuraw` captures.
#[derive(Debug)]
pub struct CaptureWriter<W: Write> {
    out: W,
    frames: u64,
    record: Vec<u8>,
}

impl CaptureWriter<BufWriter<File>> {
    /// Creates (or truncates) `path` and writes the header.
    ///
    /// # Errors
    /// [`CaptureError::Io`] if the file cannot be created or written.
    pub fn create(path: &Path, meta: &str) -> Result<Self, CaptureError> {
        Self::new(BufWriter::new(File::create(path)?), meta)
    }
}

impl<W: Write> CaptureWriter<W> {
    /// Writes the header to `out`.
    ///
    /// # Errors
    /// [`CaptureError::Io`] on write failure, [`CaptureError::Corrupt`] if `meta` is too long.
    pub fn new(mut out: W, meta: &str) -> Result<Self, CaptureError> {
        if meta.len() > MAX_META_LEN {
            return Err(CaptureError::Corrupt {
                frame: 0,
                reason: "header text longer than 64 KiB",
            });
        }
        let meta_len = u32::try_from(meta.len()).unwrap_or(u32::MAX);
        out.write_all(&MAGIC)?;
        out.write_all(&FORMAT_VERSION.to_le_bytes())?;
        out.write_all(&layout_size_u32().to_le_bytes())?;
        out.write_all(&LAYOUT_FINGERPRINT.to_le_bytes())?;
        out.write_all(&meta_len.to_le_bytes())?;
        out.write_all(meta.as_bytes())?;
        Ok(Self {
            out,
            frames: 0,
            record: Vec::new(),
        })
    }

    /// Appends one frame: `blocks` as returned by [`RawBlocks::present_blocks`] or
    /// [`crate::blocks::OwnedBlocks::blocks`].
    ///
    /// # Errors
    /// [`CaptureError::Io`] on write failure, [`CaptureError::Corrupt`] if the record is too large.
    pub fn write_frame(
        &mut self,
        info: &FrameInfo,
        blocks: &[(BlockKind, usize, &[u8])],
    ) -> Result<(), CaptureError> {
        let corrupt = |reason| CaptureError::Corrupt {
            frame: self.frames,
            reason,
        };
        let block_count = u8::try_from(blocks.len()).map_err(|_| corrupt("too many blocks"))?;
        self.record.clear();
        self.record.extend_from_slice(&info.seq.to_le_bytes());
        self.record
            .extend_from_slice(&info.capture_mono_ns.to_le_bytes());
        self.record
            .extend_from_slice(&info.connection_id.to_le_bytes());
        self.record.push(info.flags);
        self.record.push(block_count);
        for &(kind, offset, bytes) in blocks {
            let offset = u32::try_from(offset).map_err(|_| corrupt("block offset beyond 4 GiB"))?;
            let len = u32::try_from(bytes.len()).map_err(|_| corrupt("block longer than 4 GiB"))?;
            self.record.push(kind as u8);
            self.record.extend_from_slice(&offset.to_le_bytes());
            self.record.extend_from_slice(&len.to_le_bytes());
            self.record.extend_from_slice(bytes);
        }
        if self.record.len() > MAX_RECORD_LEN {
            return Err(corrupt("frame record longer than 1 MiB"));
        }
        let record_len =
            u32::try_from(self.record.len()).map_err(|_| corrupt("frame record too long"))?;
        self.out.write_all(&record_len.to_le_bytes())?;
        self.out.write_all(&self.record)?;
        self.frames += 1;
        Ok(())
    }

    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Flushes buffered frames (an interrupted recording then loses at most the unflushed tail).
    ///
    /// # Errors
    /// [`CaptureError::Io`] if the flush fails.
    pub fn flush(&mut self) -> Result<(), CaptureError> {
        self.out.flush()?;
        Ok(())
    }

    /// Flushes and returns the underlying writer.
    ///
    /// # Errors
    /// [`CaptureError::Io`] if the flush fails.
    pub fn finish(mut self) -> Result<W, CaptureError> {
        self.out.flush()?;
        Ok(self.out)
    }
}

/// One frame read back from a capture.
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    /// Snapshot sequence number at capture time.
    pub seq: u64,
    /// Capture time (the capturing process's [`crate::MonoClock`]).
    pub capture_mono_ns: u64,
    /// Connection generation of the capturing reader.
    pub connection_id: u64,
    /// [`FLAG_LOCKED`] / [`FLAG_FRAME_GATE`].
    pub flags: u8,
    /// The validated blocks.
    pub blocks: RawBlocks,
}

impl CapturedFrame {
    /// The frame source for processing: [`Consistency::Replayed`], keeping whether the frame was
    /// copied under the SDK lock, and the synchronisation of the capturing reader.
    pub fn source(&self) -> FrameSource {
        let sync = if self.flags & FLAG_FRAME_GATE != 0 {
            SyncMode::FrameGate
        } else {
            SyncMode::Polling
        };
        FrameSource {
            sync: Some(sync),
            consistency: Consistency::Replayed {
                locked: self.flags & FLAG_LOCKED != 0,
            },
        }
    }
}

/// Reads `.lmuraw` captures.
#[derive(Debug)]
pub struct CaptureReader<R: Read> {
    input: R,
    meta: String,
    frames: u64,
    record: Vec<u8>,
}

impl CaptureReader<BufReader<File>> {
    /// Opens `path` and validates the header.
    ///
    /// # Errors
    /// See [`CaptureReader::new`].
    pub fn open(path: &Path) -> Result<Self, CaptureError> {
        Self::new(BufReader::new(File::open(path)?))
    }
}

/// Reads exactly `buf.len()` bytes; `Ok(false)` on a clean end of file before the first byte.
fn read_full(input: &mut impl Read, buf: &mut [u8], frame: u64) -> Result<bool, CaptureError> {
    let mut filled = 0;
    while filled < buf.len() {
        let Some(rest) = buf.get_mut(filled..) else {
            break;
        };
        match input.read(rest) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(CaptureError::Truncated { frame }),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

/// Little-endian cursor over one frame record.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
    frame: u64,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CaptureError> {
        let end = self.pos.checked_add(n).filter(|&end| end <= self.buf.len());
        let slice = end.and_then(|end| self.buf.get(self.pos..end));
        let slice = slice.ok_or(CaptureError::Corrupt {
            frame: self.frame,
            reason: "block runs past the record",
        })?;
        self.pos += n;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CaptureError> {
        let bytes = self.take(N)?;
        <[u8; N]>::try_from(bytes).map_err(|_| CaptureError::Corrupt {
            frame: self.frame,
            reason: "short field",
        })
    }
}

impl<R: Read> CaptureReader<R> {
    /// Validates the header.
    ///
    /// # Errors
    /// [`CaptureError::BadMagic`], [`CaptureError::Version`], [`CaptureError::LayoutMismatch`],
    /// [`CaptureError::Corrupt`] or [`CaptureError::Io`].
    pub fn new(mut input: R) -> Result<Self, CaptureError> {
        let mut magic = [0u8; 8];
        if !read_full(&mut input, &mut magic, 0)? || magic != MAGIC {
            return Err(CaptureError::BadMagic);
        }
        let mut version = [0u8; 2];
        if !read_full(&mut input, &mut version, 0)? {
            return Err(CaptureError::Truncated { frame: 0 });
        }
        // The version decides the rest of the header: check it before reading further.
        let version = u16::from_le_bytes(version);
        if version != FORMAT_VERSION {
            return Err(CaptureError::Version(version));
        }
        let mut layout_size = [0u8; 4];
        let mut fingerprint = [0u8; 8];
        let mut meta_len = [0u8; 4];
        for buf in [
            &mut layout_size[..],
            &mut fingerprint[..],
            &mut meta_len[..],
        ] {
            if !read_full(&mut input, buf, 0)? {
                return Err(CaptureError::Truncated { frame: 0 });
            }
        }
        let found = u32::from_le_bytes(layout_size);
        if found != layout_size_u32() {
            return Err(CaptureError::LayoutMismatch {
                found,
                expected: layout_size_u32(),
            });
        }
        let found = u64::from_le_bytes(fingerprint);
        if found != LAYOUT_FINGERPRINT {
            return Err(CaptureError::FingerprintMismatch {
                found,
                expected: LAYOUT_FINGERPRINT,
            });
        }
        let meta_len = usize::try_from(u32::from_le_bytes(meta_len)).unwrap_or(usize::MAX);
        if meta_len > MAX_META_LEN {
            return Err(CaptureError::Corrupt {
                frame: 0,
                reason: "header text longer than 64 KiB",
            });
        }
        let mut meta = vec![0u8; meta_len];
        if !read_full(&mut input, &mut meta, 0)? && meta_len > 0 {
            return Err(CaptureError::Truncated { frame: 0 });
        }
        Ok(Self {
            input,
            meta: String::from_utf8_lossy(&meta).into_owned(),
            frames: 0,
            record: Vec::new(),
        })
    }

    /// The header text.
    pub fn meta(&self) -> &str {
        &self.meta
    }

    /// Frames read so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Reads the next frame; `Ok(None)` at the end of the file.
    ///
    /// # Errors
    /// [`CaptureError`] for I/O failures, truncation, corrupt lengths and blocks that do not fit
    /// the layout.
    pub fn next_frame(&mut self) -> Result<Option<CapturedFrame>, CaptureError> {
        let frame = self.frames;
        let mut len = [0u8; 4];
        if !read_full(&mut self.input, &mut len, frame)? {
            return Ok(None);
        }
        let record_len = usize::try_from(u32::from_le_bytes(len)).unwrap_or(usize::MAX);
        if !(FRAME_FIXED_LEN..=MAX_RECORD_LEN).contains(&record_len) {
            return Err(CaptureError::Corrupt {
                frame,
                reason: "frame record length out of range",
            });
        }
        self.record.resize(record_len, 0);
        if !read_full(&mut self.input, &mut self.record, frame)? {
            return Err(CaptureError::Truncated { frame });
        }

        let mut cur = Cursor {
            buf: &self.record,
            pos: 0,
            frame,
        };
        let seq = u64::from_le_bytes(cur.array()?);
        let capture_mono_ns = u64::from_le_bytes(cur.array()?);
        let connection_id = u64::from_le_bytes(cur.array()?);
        let [flags] = cur.array::<1>()?;
        let [block_count] = cur.array::<1>()?;
        let mut blocks = RawBlocks::new();
        blocks.begin_load();
        let mut seen = Vec::with_capacity(usize::from(block_count));
        for _ in 0..block_count {
            let [kind] = cur.array::<1>()?;
            let kind = BlockKind::from_u8(kind).ok_or(CaptureError::Corrupt {
                frame,
                reason: "unknown block kind",
            })?;
            if seen.contains(&kind) {
                return Err(CaptureError::Corrupt {
                    frame,
                    reason: "duplicate block",
                });
            }
            let offset = usize::try_from(u32::from_le_bytes(cur.array()?)).unwrap_or(usize::MAX);
            let len = usize::try_from(u32::from_le_bytes(cur.array()?)).unwrap_or(usize::MAX);
            if len > MAX_RECORD_LEN.saturating_sub(BLOCK_HEADER_LEN) {
                return Err(CaptureError::Corrupt {
                    frame,
                    reason: "block length out of range",
                });
            }
            let bytes = cur.take(len)?;
            blocks
                .load_block(kind, offset, bytes)
                .map_err(|source| CaptureError::Block { frame, source })?;
            seen.push(kind);
        }
        if cur.pos != self.record.len() {
            return Err(CaptureError::Corrupt {
                frame,
                reason: "trailing bytes after the last block",
            });
        }
        blocks
            .finish_load(&seen)
            .map_err(|source| CaptureError::Block { frame, source })?;
        self.frames += 1;
        Ok(Some(CapturedFrame {
            seq,
            capture_mono_ns,
            connection_id,
            flags,
            blocks,
        }))
    }
}

/// Replays a capture through the same [`FrameProcessor`] as the live reader.
#[derive(Debug)]
pub struct CaptureReplay<R: Read> {
    reader: CaptureReader<R>,
    processor: FrameProcessor,
    stats: ReaderStats,
    connection_id: Option<u64>,
}

impl CaptureReplay<BufReader<File>> {
    /// Opens a capture file for replay.
    ///
    /// # Errors
    /// See [`CaptureReader::new`].
    pub fn open(path: &Path) -> Result<Self, CaptureError> {
        Ok(Self::new(CaptureReader::open(path)?))
    }
}

impl<R: Read> CaptureReplay<R> {
    /// Wraps a reader.
    pub fn new(reader: CaptureReader<R>) -> Self {
        Self {
            reader,
            processor: FrameProcessor::new(),
            stats: ReaderStats::default(),
            connection_id: None,
        }
    }

    /// The header text.
    pub fn meta(&self) -> &str {
        self.reader.meta()
    }

    /// The next snapshot (byte-identical frames are skipped); `Ok(None)` at the end.
    ///
    /// # Errors
    /// Any [`CaptureError`]; a frame that fails a structural check is an error
    /// ([`CaptureError::Incompatible`]), never silently decoded.
    pub fn next_snapshot(&mut self) -> Result<Option<RawSnapshot>, CaptureError> {
        loop {
            let frame_index = self.reader.frames();
            let Some(mut frame) = self.reader.next_frame()? else {
                return Ok(None);
            };
            // A new connection in the capture (LMU re-attached): the live reader reset its processor
            // there, so freshness and duplicate detection restart exactly as they did live.
            if self.connection_id != Some(frame.connection_id) {
                if self.connection_id.is_some() {
                    self.processor.reset();
                }
                self.connection_id = Some(frame.connection_id);
            }
            let meta = FrameMeta {
                capture_mono_ns: frame.capture_mono_ns,
                connection_id: frame.connection_id,
                source: frame.source(),
                mapping_size: None,
            };
            let outcome = self
                .processor
                .process(&mut frame.blocks, &meta, &mut self.stats)
                .map_err(|source| CaptureError::Decode {
                    frame: frame_index,
                    source,
                })?;
            match outcome {
                FrameOutcome::Snapshot(snapshot) => return Ok(Some(*snapshot)),
                FrameOutcome::Duplicate => {}
                FrameOutcome::Rejected => {
                    return Err(CaptureError::Incompatible {
                        frame: frame_index,
                        message: self.processor.report().incompatibility_message(),
                    });
                }
            }
        }
    }

    /// Statistics of the replay (duplicates, rates in capture time, ...).
    pub fn stats(&self) -> &ReaderStats {
        &self.stats
    }

    /// Compatibility report over the replayed frames.
    pub fn compatibility(&self) -> CompatibilityReport {
        self.processor.report()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::{CopyPlan, copy_frame};
    use crate::testutil::LayoutBuilder;

    const LIVE_FLAGS: u8 = FLAG_LOCKED | FLAG_FRAME_GATE;
    const FRAME_INTERVAL_NS: u64 = 10_000_000;

    /// Writes one record per `(connection_id, image)`.
    fn capture_with(frames: &[(u64, &LayoutBuilder)], plan: CopyPlan, flags: u8) -> Vec<u8> {
        let mut w = CaptureWriter::new(Vec::new(), "tool: test\n").expect("header");
        let mut blocks = RawBlocks::new();
        blocks.enable_scoring_stream();
        for (i, (connection_id, b)) in frames.iter().enumerate() {
            copy_frame(b.bytes(), plan, &mut blocks).expect("copy");
            let seq = u64::try_from(i).expect("index");
            let info = FrameInfo {
                seq: seq + 1,
                capture_mono_ns: seq * FRAME_INTERVAL_NS,
                connection_id: *connection_id,
                flags,
            };
            w.write_frame(&info, &blocks.present_blocks())
                .expect("frame");
        }
        assert_eq!(w.frames(), u64::try_from(frames.len()).expect("len"));
        w.finish().expect("flush")
    }

    fn capture(frames: &[LayoutBuilder], plan: CopyPlan) -> Vec<u8> {
        let frames: Vec<(u64, &LayoutBuilder)> = frames.iter().map(|b| (1, b)).collect();
        capture_with(&frames, plan, LIVE_FLAGS)
    }

    fn header_len() -> usize {
        MAGIC.len() + 2 + 4 + 8 + 4 + "tool: test\n".len()
    }

    #[test]
    fn round_trip_preserves_blocks_and_metadata() {
        let mut b = LayoutBuilder::on_track_frame();
        b.scoring_stream_size(4);
        b.put(crate::offsets::abs::SCORING_STREAM, b"a\nb\n");
        let plan = CopyPlan {
            paths: true,
            scoring_stream: true,
        };
        let bytes = capture(std::slice::from_ref(&b), plan);
        let mut r = CaptureReader::new(bytes.as_slice()).expect("header");
        assert_eq!(r.meta(), "tool: test\n");
        let frame = r.next_frame().expect("read").expect("one frame");
        assert_eq!(
            (
                frame.seq,
                frame.capture_mono_ns,
                frame.connection_id,
                frame.flags
            ),
            (1, 0, 1, LIVE_FLAGS)
        );
        let mut expected = RawBlocks::new();
        expected.enable_scoring_stream();
        copy_frame(b.bytes(), plan, &mut expected).expect("copy");
        assert_eq!(frame.blocks.present_blocks(), expected.present_blocks());
        assert_eq!(
            frame.blocks.telemetry_count(),
            2,
            "every telemetry slot is recorded, not only the player's"
        );
        assert!(r.next_frame().expect("eof").is_none());
    }

    /// Runs the frames through a processor exactly as the live reader does (same metadata apart
    /// from the source) and returns the snapshots.
    fn live_snapshots(frames: &[(u64, &LayoutBuilder)]) -> Vec<RawSnapshot> {
        let mut processor = FrameProcessor::new();
        let mut stats = ReaderStats::default();
        let mut work = RawBlocks::new();
        let mut connection = None;
        let mut out = Vec::new();
        for (i, (connection_id, b)) in frames.iter().enumerate() {
            if connection.is_some_and(|c| c != *connection_id) {
                processor.reset(); // LmuReader::try_connect resets on every attach
            }
            connection = Some(*connection_id);
            copy_frame(b.bytes(), CopyPlan::default(), &mut work).expect("copy");
            let meta = FrameMeta {
                capture_mono_ns: u64::try_from(i).expect("index") * FRAME_INTERVAL_NS,
                connection_id: *connection_id,
                source: FrameSource {
                    sync: Some(SyncMode::FrameGate),
                    consistency: Consistency::Locked,
                },
                mapping_size: Some(layout::SIZE),
            };
            if let FrameOutcome::Snapshot(s) = processor
                .process(&mut work, &meta, &mut stats)
                .expect("process")
            {
                out.push(*s);
            }
        }
        out
    }

    fn replay_all(bytes: &[u8]) -> Vec<RawSnapshot> {
        let mut replay = CaptureReplay::new(CaptureReader::new(bytes).expect("header"));
        let mut out = Vec::new();
        while let Some(s) = replay.next_snapshot().expect("replay") {
            out.push(s);
        }
        out
    }

    #[test]
    fn replay_yields_the_same_snapshots_as_live_processing() {
        let first = LayoutBuilder::on_track_frame();
        let mut second = first.clone();
        second.player_elapsed_time(0, 120.01);
        let mut third = second.clone();
        third.scoring_et(120.2);
        // A duplicate, then a reconnect whose first frame is byte-identical to the last one.
        let frames = [
            (1, &first),
            (1, &first),
            (1, &second),
            (1, &third),
            (2, &third),
        ];
        let live = live_snapshots(&frames);
        let replayed = replay_all(&capture_with(&frames, CopyPlan::default(), LIVE_FLAGS));
        assert_eq!(
            live.len(),
            4,
            "the duplicate is dropped, the reconnect is not"
        );
        assert_eq!(replayed.len(), live.len());
        for (live, mut replayed) in live.into_iter().zip(replayed) {
            assert_eq!(
                replayed.source,
                FrameSource {
                    sync: Some(SyncMode::FrameGate),
                    consistency: Consistency::Replayed { locked: true },
                }
            );
            replayed.source = live.source;
            assert_eq!(replayed, live);
        }
    }

    #[test]
    fn replay_resets_freshness_on_a_new_connection_and_keeps_lock_provenance() {
        let b = LayoutBuilder::on_track_frame();
        let bytes = capture_with(&[(4, &b), (5, &b)], CopyPlan::default(), FLAG_FRAME_GATE);
        let snapshots = replay_all(&bytes);
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[1].connection_id, 5);
        assert!(snapshots[1].telemetry_updated && snapshots[1].scoring_updated);
        assert_eq!(
            snapshots[0].source.consistency,
            Consistency::Replayed { locked: false },
            "a lock-free (degraded) copy stays recognisable after replay"
        );
    }

    #[test]
    fn replay_refuses_structurally_incompatible_frames() {
        let mut b = LayoutBuilder::on_track_frame();
        b.scoring_padding([0, 0, 0, 9]);
        let bytes = capture(&[b], CopyPlan::default());
        let mut replay = CaptureReplay::new(CaptureReader::new(bytes.as_slice()).expect("header"));
        let err = replay.next_snapshot().expect_err("must refuse");
        assert!(
            matches!(err, CaptureError::Incompatible { frame: 0, .. }),
            "{err}"
        );
    }

    #[test]
    fn header_problems_are_reported() {
        assert!(matches!(
            CaptureReader::new(&b"NOTRAW!!"[..]),
            Err(CaptureError::BadMagic)
        ));
        let mut bytes = capture(&[], CopyPlan::default());
        bytes[8] = 1;
        assert!(
            matches!(
                CaptureReader::new(bytes.as_slice()),
                Err(CaptureError::Version(1))
            ),
            "version-1 captures (player slot only, no fingerprint) are refused"
        );
        let mut bytes = capture(&[], CopyPlan::default());
        bytes[10..14].copy_from_slice(&324_816u32.to_le_bytes());
        assert!(matches!(
            CaptureReader::new(bytes.as_slice()),
            Err(CaptureError::LayoutMismatch { found: 324_816, .. })
        ));
        // Same size, different field layout (a re-probe moved fields): refused.
        let mut bytes = capture(&[], CopyPlan::default());
        bytes[14..22].copy_from_slice(&(LAYOUT_FINGERPRINT ^ 1).to_le_bytes());
        assert!(matches!(
            CaptureReader::new(bytes.as_slice()),
            Err(CaptureError::FingerprintMismatch { .. })
        ));
    }

    #[test]
    fn truncated_and_corrupt_records_are_errors_not_panics() {
        let bytes = capture(&[LayoutBuilder::on_track_frame()], CopyPlan::default());
        let header_len = header_len();
        for cut in [header_len + 2, header_len + 30, bytes.len() - 1] {
            let mut r = CaptureReader::new(&bytes[..cut]).expect("header");
            assert!(
                matches!(r.next_frame(), Err(CaptureError::Truncated { frame: 0 })),
                "cut at {cut}"
            );
        }
        let mut huge = bytes.clone();
        huge[header_len..header_len + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let mut r = CaptureReader::new(huge.as_slice()).expect("header");
        assert!(matches!(r.next_frame(), Err(CaptureError::Corrupt { .. })));
        let mut bad_kind = bytes;
        bad_kind[header_len + 4 + FRAME_FIXED_LEN] = 42;
        let mut r = CaptureReader::new(bad_kind.as_slice()).expect("header");
        assert!(matches!(
            r.next_frame(),
            Err(CaptureError::Corrupt {
                reason: "unknown block kind",
                ..
            })
        ));
    }

    /// Recomputes `LAYOUT_FINGERPRINT` from the probe JSON exactly as `codegen/gen_layout.py` does.
    #[test]
    fn the_test_binary_belongs_to_this_checkout() {
        // The probe tests read the layout JSON relative to the compile-time manifest dir. A test
        // binary left over from a moved or copied workspace would read (or fail to read) another
        // checkout's JSON; `cargo test` runs tests from the package root, so compare the two.
        let compiled = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let running = std::env::current_dir().expect("current dir");
        assert_eq!(
            compiled.canonicalize().ok(),
            running.canonicalize().ok(),
            "stale test binary: built for {} but run from {}; rebuild lmu-reader (touch src/lib.rs)",
            compiled.display(),
            running.display()
        );
    }

    #[test]
    fn layout_fingerprint_matches_the_probe() {
        const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/research/lmu_abi_layout.json");
        let text = std::fs::read_to_string(&path).expect("probe JSON");
        let json: serde_json::Value = serde_json::from_str(&text).expect("parse");
        let structs = json["structs"].as_object().expect("structs");
        let mut names: Vec<&String> = structs.keys().collect();
        names.sort();
        let mut canonical = String::new();
        for name in names {
            let s = &structs[name.as_str()];
            canonical.push_str(&format!("{name}:{}\n", s["sizeof"]));
            for f in s["fields"].as_array().expect("fields") {
                canonical.push_str(&format!(
                    "{}@{}+{}:{}\n",
                    f["name"].as_str().expect("name"),
                    f["offset"],
                    f["size"],
                    f["type"].as_str().expect("type")
                ));
            }
        }
        let hash = canonical.bytes().fold(FNV_OFFSET, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(FNV_PRIME)
        });
        assert_eq!(
            hash, LAYOUT_FINGERPRINT,
            "offsets.rs is stale: re-run codegen/gen_layout.py"
        );
    }
}

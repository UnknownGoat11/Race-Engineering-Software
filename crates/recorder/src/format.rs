//! `.lmurec` on-disk format: file header, chunk header, record framing and recorder notes.
//!
//! Pure byte-level code: no I/O, no clocks. The normative description (with byte tables) is
//! `docs/STORAGE_DESIGN.md` §3; every offset below must match it.
//!
//! All integers are little-endian.

use thiserror::Error;
use uuid::Uuid;

/// File magic: `0x89 'L' 'M' 'U' 'R' 'E' 'C' '\n'`. The high-bit first byte catches 7-bit transfers and the
/// trailing `\n` catches text-mode line-ending conversion (same idea as the PNG signature).
pub const FILE_MAGIC: [u8; 8] = [0x89, b'L', b'M', b'U', b'R', b'E', b'C', b'\n'];
/// Current (and only) recording format version written by this crate.
pub const FORMAT_VERSION: u16 = 1;
/// Size of the fixed v1 file header in bytes (including its trailing CRC).
pub const FILE_HEADER_LEN: usize = 96;
/// Maximum UTF-8 length of the application version string stored in the file header.
pub const MAX_APP_VERSION_BYTES: usize = 35;

/// Chunk magic: ASCII `LRCH`.
pub const CHUNK_MAGIC: [u8; 4] = *b"LRCH";
/// Size of the fixed chunk header in bytes (including both CRC fields).
pub const CHUNK_HEADER_LEN: usize = 48;
/// Size of the per-record header inside a decompressed chunk body: kind u8 | capture_ns u64 | len u32.
pub const RECORD_HEADER_LEN: usize = 1 + 8 + 4;

/// Largest payload accepted for a single record (a full raw `LMU_Data` copy is ~325 KB; this leaves
/// generous headroom while keeping a hostile/corrupt length from causing a huge allocation).
pub const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;
/// Largest decompressed chunk body a reader will accept (bounds reader memory).
pub const MAX_CHUNK_RAW_BYTES: usize = 64 * 1024 * 1024;
/// Largest compressed chunk body a reader will accept. zstd's worst-case expansion is
/// `n + n/256 + 64` for large inputs (ZSTD_COMPRESSBOUND); `n/128 + 4096` is a strict superset.
pub const MAX_CHUNK_BODY_BYTES: usize = MAX_CHUNK_RAW_BYTES + MAX_CHUNK_RAW_BYTES / 128 + 4096;

// Compile-time guarantees the length fields rely on: one maximal record fits in a chunk, and every chunk
// length fits the u32 header fields (the `as u32` conversions in the writer depend on this).
const _: () = assert!(MAX_RECORD_BYTES + RECORD_HEADER_LEN <= MAX_CHUNK_RAW_BYTES);
const _: () = assert!(MAX_CHUNK_BODY_BYTES <= u32::MAX as usize);

/// Sentinel stored in `created_mono_ns` when no capture clock was supplied.
pub const MONO_NS_UNKNOWN: u64 = u64::MAX;

/// Largest `capture_ns` the recorder accepts (`i64::MAX`, ≈292 years of monotonic time): the session
/// index stores capture times in SQLite's signed 64-bit INTEGER, and ordering must survive the round trip.
pub const MAX_CAPTURE_NS: u64 = i64::MAX as u64;

// File header field offsets (docs/STORAGE_DESIGN.md §3.1).
const FH_MAGIC: usize = 0;
const FH_FORMAT_VERSION: usize = 8;
const FH_HEADER_LEN: usize = 10;
const FH_SCHEMA_VERSION: usize = 12;
const FH_FLAGS: usize = 14;
const FH_SESSION_ID: usize = 16;
const FH_SEGMENT_INDEX: usize = 32;
const FH_RESERVED: usize = 36;
const FH_CREATED_UNIX_NS: usize = 40;
const FH_CREATED_MONO_NS: usize = 48;
const FH_APP_VERSION_LEN: usize = 56;
const FH_APP_VERSION: usize = 57;
const FH_CRC: usize = 92;

// Chunk header field offsets (docs/STORAGE_DESIGN.md §3.2).
const CH_MAGIC: usize = 0;
const CH_BODY_LEN: usize = 4;
const CH_RECORD_COUNT: usize = 8;
const CH_SEQ: usize = 12;
const CH_MIN_NS: usize = 16;
const CH_MAX_NS: usize = 24;
const CH_RAW_LEN: usize = 32;
const CH_BODY_CRC: usize = 36;
const CH_CODEC: usize = 40;
const CH_RESERVED: usize = 41;
const CH_RESERVED_LEN: usize = 3;
/// Offset of the header CRC; it covers header bytes `[0, CH_HEADER_CRC)`, which include the body CRC.
const CH_HEADER_CRC: usize = 44;

/// Errors decoding or validating `.lmurec` structures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FormatError {
    #[error("bad file magic {found:02x?}")]
    BadFileMagic { found: [u8; 8] },
    #[error("unsupported recording format version {found} (this build reads {FORMAT_VERSION})")]
    UnsupportedVersion { found: u16 },
    #[error("file header length field is {found}, expected {FILE_HEADER_LEN}")]
    BadHeaderLength { found: u16 },
    #[error("file header CRC mismatch: stored {stored:#010x}, computed {computed:#010x}")]
    HeaderCrc { stored: u32, computed: u32 },
    #[error("file header truncated: {available} of {FILE_HEADER_LEN} bytes present")]
    HeaderTruncated { available: usize },
    #[error("app version string is {len} bytes, maximum is {MAX_APP_VERSION_BYTES}")]
    AppVersionTooLong { len: usize },
    #[error("file header app version is not valid UTF-8")]
    AppVersionUtf8,
    #[error("non-zero reserved bytes in {structure} header")]
    ReservedNonZero { structure: &'static str },
    #[error("bad chunk magic {found:02x?}")]
    BadChunkMagic { found: [u8; 4] },
    #[error("chunk header CRC mismatch: stored {stored:#010x}, computed {computed:#010x}")]
    ChunkHeaderCrc { stored: u32, computed: u32 },
    #[error("chunk body CRC mismatch: stored {stored:#010x}, computed {computed:#010x}")]
    ChunkBodyCrc { stored: u32, computed: u32 },
    #[error("chunk body length {len} exceeds maximum {MAX_CHUNK_BODY_BYTES}")]
    ChunkBodyTooLarge { len: u32 },
    #[error("chunk raw length {len} exceeds maximum {MAX_CHUNK_RAW_BYTES}")]
    ChunkRawTooLarge { len: u32 },
    #[error("unknown chunk codec {code}")]
    UnknownCodec { code: u8 },
    #[error("chunk header is inconsistent: {reason}")]
    ChunkHeaderInconsistent { reason: &'static str },
    #[error("chunk sequence {found} where {expected} was expected")]
    ChunkSequence { expected: u32, found: u32 },
    #[error("chunk body is {actual} bytes, header says {expected}")]
    BodyLengthMismatch { expected: u32, actual: usize },
    #[error("chunk body failed to decompress: {message}")]
    Decompress { message: String },
    #[error("chunk decompressed to {actual} bytes, header says {expected}")]
    RawLengthMismatch { expected: u32, actual: usize },
    #[error("record at body offset {offset} is truncated")]
    RecordTruncated { offset: usize },
    #[error("record kind code {code} is invalid")]
    InvalidRecordKind { code: u8 },
    #[error("record payload is {len} bytes, maximum is {MAX_RECORD_BYTES}")]
    RecordTooLarge { len: usize },
    #[error("chunk holds {actual} records, header says {expected}")]
    RecordCountMismatch { expected: u32, actual: u32 },
    #[error("record capture time {capture_ns} outside chunk range [{min_ns}, {max_ns}]")]
    RecordOutsideChunkRange {
        capture_ns: u64,
        min_ns: u64,
        max_ns: u64,
    },
    #[error("gap note is malformed: {reason}")]
    BadGapNote { reason: &'static str },
}

/// What a record's payload contains. The recorder is payload-agnostic; the kind tells readers which
/// decoder to use. Codes are append-only and never reused (docs/STORAGE_DESIGN.md §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RecordKind {
    /// Canonical telemetry as a telemetry-model TLV block (docs/PROTOCOL.md §4).
    TelemetryTlv,
    /// Scoring/standings update, postcard-encoded (`SCORING_UPDATE` payload).
    ScoringPostcard,
    /// Weather channels as a TLV block (`WEATHER_UPDATE` payload).
    WeatherTlv,
    /// Session event (lap/sector/pit/flag/LMU lost…), postcard-encoded (`SESSION_EVENT` payload).
    SessionEvent,
    /// Raw bytes copied from `LMU_Data` (producer-defined region header + bytes; the producer redacts
    /// personal data before recording).
    RawLmuBlock,
    /// Application marker (user bookmark, app-level annotation).
    Marker,
    /// Capability map / schema negotiation payload (`SCHEMA_NEGOTIATE`), postcard-encoded.
    CapabilityPostcard,
    /// Written only by the recorder itself (gap notes, see [`GapNote`]). Rejected from `append`.
    RecorderNote,
    /// A code this build does not know (written by a newer version). Only produced by
    /// [`RecordKind::from_code`]; preserved, never reinterpreted, and rejected from `append`.
    Unknown(u8),
}

impl RecordKind {
    /// Stable on-disk code. Code 0 is reserved and never valid.
    pub const fn code(self) -> u8 {
        match self {
            RecordKind::TelemetryTlv => 1,
            RecordKind::ScoringPostcard => 2,
            RecordKind::WeatherTlv => 3,
            RecordKind::SessionEvent => 4,
            RecordKind::RawLmuBlock => 5,
            RecordKind::Marker => 6,
            RecordKind::CapabilityPostcard => 7,
            RecordKind::RecorderNote => 8,
            RecordKind::Unknown(c) => c,
        }
    }

    /// Maps an on-disk code to a kind. Returns `None` for the reserved code 0.
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            0 => return None,
            1 => RecordKind::TelemetryTlv,
            2 => RecordKind::ScoringPostcard,
            3 => RecordKind::WeatherTlv,
            4 => RecordKind::SessionEvent,
            5 => RecordKind::RawLmuBlock,
            6 => RecordKind::Marker,
            7 => RecordKind::CapabilityPostcard,
            8 => RecordKind::RecorderNote,
            other => RecordKind::Unknown(other),
        })
    }

    /// Whether producers may pass this kind to the recorder (`RecorderNote` and `Unknown` are reserved).
    pub const fn is_producer_kind(self) -> bool {
        !matches!(self, RecordKind::RecorderNote | RecordKind::Unknown(_))
    }

    /// Stable upper-case name for logs and diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            RecordKind::TelemetryTlv => "TELEMETRY_TLV",
            RecordKind::ScoringPostcard => "SCORING_POSTCARD",
            RecordKind::WeatherTlv => "WEATHER_TLV",
            RecordKind::SessionEvent => "SESSION_EVENT",
            RecordKind::RawLmuBlock => "RAW_LMU_BLOCK",
            RecordKind::Marker => "MARKER",
            RecordKind::CapabilityPostcard => "CAPABILITY_POSTCARD",
            RecordKind::RecorderNote => "RECORDER_NOTE",
            RecordKind::Unknown(_) => "UNKNOWN",
        }
    }
}

/// Fixed header at the start of every `.lmurec` segment file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHeader {
    /// Recording format version ([`FORMAT_VERSION`]).
    pub format_version: u16,
    /// `TELEMETRY_SCHEMA_VERSION` of the producer that wrote the TLV records.
    pub telemetry_schema_version: u16,
    /// Session the segment belongs to.
    pub session_id: Uuid,
    /// 0-based position of this segment within its session.
    pub segment_index: u32,
    /// Wall clock at segment creation, ns since the Unix epoch.
    pub created_unix_ns: i64,
    /// Capture (monotonic) clock at segment creation, or [`MONO_NS_UNKNOWN`]. Together with
    /// `created_unix_ns` this anchors `capture_ns` values to wall-clock time.
    pub created_mono_ns: u64,
    /// Application version string (≤ [`MAX_APP_VERSION_BYTES`] bytes).
    pub app_version: String,
}

impl FileHeader {
    /// Serializes the header into its fixed 96-byte form.
    pub fn encode(&self) -> Result<[u8; FILE_HEADER_LEN], FormatError> {
        let app = self.app_version.as_bytes();
        if app.len() > MAX_APP_VERSION_BYTES {
            return Err(FormatError::AppVersionTooLong { len: app.len() });
        }
        let mut b = [0u8; FILE_HEADER_LEN];
        b[FH_MAGIC..FH_MAGIC + 8].copy_from_slice(&FILE_MAGIC);
        b[FH_FORMAT_VERSION..FH_FORMAT_VERSION + 2]
            .copy_from_slice(&self.format_version.to_le_bytes());
        b[FH_HEADER_LEN..FH_HEADER_LEN + 2]
            .copy_from_slice(&(FILE_HEADER_LEN as u16).to_le_bytes());
        b[FH_SCHEMA_VERSION..FH_SCHEMA_VERSION + 2]
            .copy_from_slice(&self.telemetry_schema_version.to_le_bytes());
        // FH_FLAGS (u16) and FH_RESERVED (u32) stay zero in v1.
        b[FH_SESSION_ID..FH_SESSION_ID + 16].copy_from_slice(self.session_id.as_bytes());
        b[FH_SEGMENT_INDEX..FH_SEGMENT_INDEX + 4]
            .copy_from_slice(&self.segment_index.to_le_bytes());
        b[FH_CREATED_UNIX_NS..FH_CREATED_UNIX_NS + 8]
            .copy_from_slice(&self.created_unix_ns.to_le_bytes());
        b[FH_CREATED_MONO_NS..FH_CREATED_MONO_NS + 8]
            .copy_from_slice(&self.created_mono_ns.to_le_bytes());
        // Length checked above to be ≤ 35, so it fits in u8.
        b[FH_APP_VERSION_LEN] = app.len() as u8;
        b[FH_APP_VERSION..FH_APP_VERSION + app.len()].copy_from_slice(app);
        let crc = crc32c::crc32c(&b[..FH_CRC]);
        b[FH_CRC..FH_CRC + 4].copy_from_slice(&crc.to_le_bytes());
        Ok(b)
    }

    /// Parses and validates a file header. `bytes` may be longer than the header; only the first
    /// [`FILE_HEADER_LEN`] bytes are examined.
    ///
    /// The version is checked before the CRC so a file written by a newer format version is reported
    /// as "unsupported version" rather than as corrupt.
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        if bytes.len() < FILE_HEADER_LEN {
            // Report a magic mismatch first when enough bytes exist to tell: a short foreign file is
            // "not ours", a short file with our magic is "truncated".
            if bytes.len() >= FILE_MAGIC.len() && bytes[..FILE_MAGIC.len()] != FILE_MAGIC {
                return Err(FormatError::BadFileMagic {
                    found: arr(bytes, FH_MAGIC),
                });
            }
            return Err(FormatError::HeaderTruncated {
                available: bytes.len(),
            });
        }
        let b = &bytes[..FILE_HEADER_LEN];
        let magic: [u8; 8] = arr(b, FH_MAGIC);
        if magic != FILE_MAGIC {
            return Err(FormatError::BadFileMagic { found: magic });
        }
        let format_version = u16::from_le_bytes(arr(b, FH_FORMAT_VERSION));
        if format_version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion {
                found: format_version,
            });
        }
        let header_len = u16::from_le_bytes(arr(b, FH_HEADER_LEN));
        if usize::from(header_len) != FILE_HEADER_LEN {
            return Err(FormatError::BadHeaderLength { found: header_len });
        }
        let stored = u32::from_le_bytes(arr(b, FH_CRC));
        let computed = crc32c::crc32c(&b[..FH_CRC]);
        if stored != computed {
            return Err(FormatError::HeaderCrc { stored, computed });
        }
        let flags = u16::from_le_bytes(arr(b, FH_FLAGS));
        let reserved = u32::from_le_bytes(arr(b, FH_RESERVED));
        if flags != 0 || reserved != 0 {
            return Err(FormatError::ReservedNonZero { structure: "file" });
        }
        let app_len = usize::from(b[FH_APP_VERSION_LEN]);
        if app_len > MAX_APP_VERSION_BYTES {
            return Err(FormatError::AppVersionTooLong { len: app_len });
        }
        let app_version = std::str::from_utf8(&b[FH_APP_VERSION..FH_APP_VERSION + app_len])
            .map_err(|_| FormatError::AppVersionUtf8)?
            .to_owned();
        Ok(Self {
            format_version,
            telemetry_schema_version: u16::from_le_bytes(arr(b, FH_SCHEMA_VERSION)),
            session_id: Uuid::from_bytes(arr(b, FH_SESSION_ID)),
            segment_index: u32::from_le_bytes(arr(b, FH_SEGMENT_INDEX)),
            created_unix_ns: i64::from_le_bytes(arr(b, FH_CREATED_UNIX_NS)),
            created_mono_ns: u64::from_le_bytes(arr(b, FH_CREATED_MONO_NS)),
            app_version,
        })
    }

    /// `created_mono_ns` as an option (the sentinel maps to `None`).
    pub fn created_mono_ns(&self) -> Option<u64> {
        (self.created_mono_ns != MONO_NS_UNKNOWN).then_some(self.created_mono_ns)
    }
}

/// How a chunk body is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkCodec {
    /// Body is the raw record bytes: used when zstd does not shrink the data (tiny or incompressible
    /// chunks, a normal case) or when compression fails. Not an error signal.
    Stored,
    /// Body is one zstd frame of the raw record bytes.
    Zstd,
}

impl ChunkCodec {
    /// Stable on-disk code.
    pub const fn code(self) -> u8 {
        match self {
            ChunkCodec::Stored => 0,
            ChunkCodec::Zstd => 1,
        }
    }

    /// Maps an on-disk code to a codec.
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(ChunkCodec::Stored),
            1 => Some(ChunkCodec::Zstd),
            _ => None,
        }
    }
}

/// Fixed header preceding every chunk body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkHeader {
    /// Length of the encoded body that follows the header ("chunk_len").
    pub body_len: u32,
    /// Number of records in the body (≥ 1).
    pub record_count: u32,
    /// 0-based sequence of the chunk within its segment.
    pub chunk_seq: u32,
    /// Smallest `capture_ns` of any record in the chunk ("first capture ns").
    pub min_capture_ns: u64,
    /// Largest `capture_ns` of any record in the chunk ("last capture ns").
    pub max_capture_ns: u64,
    /// Decoded (decompressed) body length.
    pub raw_len: u32,
    /// CRC-32C of the encoded body bytes.
    pub body_crc32c: u32,
    /// Body encoding.
    pub codec: ChunkCodec,
}

impl ChunkHeader {
    /// Total on-disk length of the chunk (header + body).
    pub fn total_len(&self) -> u64 {
        CHUNK_HEADER_LEN as u64 + u64::from(self.body_len)
    }

    /// Serializes the header, computing the header CRC over bytes `[0, 44)`.
    pub fn encode(&self) -> [u8; CHUNK_HEADER_LEN] {
        let mut b = [0u8; CHUNK_HEADER_LEN];
        b[CH_MAGIC..CH_MAGIC + 4].copy_from_slice(&CHUNK_MAGIC);
        b[CH_BODY_LEN..CH_BODY_LEN + 4].copy_from_slice(&self.body_len.to_le_bytes());
        b[CH_RECORD_COUNT..CH_RECORD_COUNT + 4].copy_from_slice(&self.record_count.to_le_bytes());
        b[CH_SEQ..CH_SEQ + 4].copy_from_slice(&self.chunk_seq.to_le_bytes());
        b[CH_MIN_NS..CH_MIN_NS + 8].copy_from_slice(&self.min_capture_ns.to_le_bytes());
        b[CH_MAX_NS..CH_MAX_NS + 8].copy_from_slice(&self.max_capture_ns.to_le_bytes());
        b[CH_RAW_LEN..CH_RAW_LEN + 4].copy_from_slice(&self.raw_len.to_le_bytes());
        b[CH_BODY_CRC..CH_BODY_CRC + 4].copy_from_slice(&self.body_crc32c.to_le_bytes());
        b[CH_CODEC] = self.codec.code();
        // CH_RESERVED (3 bytes) stays zero in v1.
        let crc = crc32c::crc32c(&b[..CH_HEADER_CRC]);
        b[CH_HEADER_CRC..CH_HEADER_CRC + 4].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Parses a chunk header: magic, header CRC, then every field that can be checked without the
    /// body. Because the header CRC is verified first, a header that decodes successfully has a
    /// trustworthy `body_len`, which is what lets a scanner tell a torn tail from mid-file corruption.
    pub fn decode(b: &[u8; CHUNK_HEADER_LEN]) -> Result<Self, FormatError> {
        let magic: [u8; 4] = arr(b, CH_MAGIC);
        if magic != CHUNK_MAGIC {
            return Err(FormatError::BadChunkMagic { found: magic });
        }
        let stored = u32::from_le_bytes(arr(b, CH_HEADER_CRC));
        let computed = crc32c::crc32c(&b[..CH_HEADER_CRC]);
        if stored != computed {
            return Err(FormatError::ChunkHeaderCrc { stored, computed });
        }
        let codec = ChunkCodec::from_code(b[CH_CODEC])
            .ok_or(FormatError::UnknownCodec { code: b[CH_CODEC] })?;
        if b[CH_RESERVED..CH_RESERVED + CH_RESERVED_LEN]
            .iter()
            .any(|&x| x != 0)
        {
            return Err(FormatError::ReservedNonZero { structure: "chunk" });
        }
        let h = Self {
            body_len: u32::from_le_bytes(arr(b, CH_BODY_LEN)),
            record_count: u32::from_le_bytes(arr(b, CH_RECORD_COUNT)),
            chunk_seq: u32::from_le_bytes(arr(b, CH_SEQ)),
            min_capture_ns: u64::from_le_bytes(arr(b, CH_MIN_NS)),
            max_capture_ns: u64::from_le_bytes(arr(b, CH_MAX_NS)),
            raw_len: u32::from_le_bytes(arr(b, CH_RAW_LEN)),
            body_crc32c: u32::from_le_bytes(arr(b, CH_BODY_CRC)),
            codec,
        };
        if h.body_len as usize > MAX_CHUNK_BODY_BYTES {
            return Err(FormatError::ChunkBodyTooLarge { len: h.body_len });
        }
        if h.raw_len as usize > MAX_CHUNK_RAW_BYTES {
            return Err(FormatError::ChunkRawTooLarge { len: h.raw_len });
        }
        if h.body_len == 0 {
            return Err(FormatError::ChunkHeaderInconsistent {
                reason: "body_len is 0",
            });
        }
        if h.record_count == 0 {
            return Err(FormatError::ChunkHeaderInconsistent {
                reason: "record_count is 0",
            });
        }
        if u64::from(h.record_count) * RECORD_HEADER_LEN as u64 > u64::from(h.raw_len) {
            return Err(FormatError::ChunkHeaderInconsistent {
                reason: "record_count exceeds raw_len capacity",
            });
        }
        if h.min_capture_ns > h.max_capture_ns {
            return Err(FormatError::ChunkHeaderInconsistent {
                reason: "min_capture_ns > max_capture_ns",
            });
        }
        if h.codec == ChunkCodec::Stored && h.body_len != h.raw_len {
            return Err(FormatError::ChunkHeaderInconsistent {
                reason: "stored chunk with body_len != raw_len",
            });
        }
        Ok(h)
    }

    /// Verifies the encoded body against `body_len` and `body_crc32c`.
    pub fn verify_body(&self, body: &[u8]) -> Result<(), FormatError> {
        if body.len() != self.body_len as usize {
            return Err(FormatError::BodyLengthMismatch {
                expected: self.body_len,
                actual: body.len(),
            });
        }
        let computed = crc32c::crc32c(body);
        if computed != self.body_crc32c {
            return Err(FormatError::ChunkBodyCrc {
                stored: self.body_crc32c,
                computed,
            });
        }
        Ok(())
    }
}

/// Appends one record (`kind u8 | capture_ns u64 | len u32 | bytes`) to a raw chunk body.
pub fn encode_record(
    buf: &mut Vec<u8>,
    kind: RecordKind,
    capture_ns: u64,
    bytes: &[u8],
) -> Result<(), FormatError> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(FormatError::RecordTooLarge { len: bytes.len() });
    }
    let code = kind.code();
    if code == 0 {
        return Err(FormatError::InvalidRecordKind { code });
    }
    buf.push(code);
    buf.extend_from_slice(&capture_ns.to_le_bytes());
    // Checked above: len ≤ MAX_RECORD_BYTES < u32::MAX.
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
    Ok(())
}

/// A record borrowed from a decoded chunk body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordRef<'a> {
    /// What the payload contains.
    pub kind: RecordKind,
    /// Capture time the producer supplied.
    pub capture_ns: u64,
    /// The payload exactly as appended.
    pub bytes: &'a [u8],
}

/// Iterator over the records of a decoded chunk body. Yields an error (and then stops) on any
/// framing problem; a well-formed body is consumed exactly.
pub struct RecordIter<'a> {
    body: &'a [u8],
    pos: usize,
    failed: bool,
}

impl<'a> RecordIter<'a> {
    /// Iterates the records of `body` (a decoded chunk body).
    pub fn new(body: &'a [u8]) -> Self {
        Self {
            body,
            pos: 0,
            failed: false,
        }
    }
}

impl<'a> Iterator for RecordIter<'a> {
    type Item = Result<RecordRef<'a>, FormatError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.pos >= self.body.len() {
            return None;
        }
        match record_at(self.body, self.pos) {
            Ok((rec, next)) => {
                self.pos = next;
                Some(Ok(rec))
            }
            Err(e) => {
                self.failed = true;
                Some(Err(e))
            }
        }
    }
}

/// Decodes the record that starts at byte `pos` of a decoded chunk body. Returns the record and the offset
/// of the next one.
pub fn record_at(body: &[u8], pos: usize) -> Result<(RecordRef<'_>, usize), FormatError> {
    let rest = body
        .get(pos..)
        .ok_or(FormatError::RecordTruncated { offset: pos })?;
    if rest.len() < RECORD_HEADER_LEN {
        return Err(FormatError::RecordTruncated { offset: pos });
    }
    // Record header: kind u8 @0 | capture_ns u64 @1 | len u32 @9 (docs/STORAGE_DESIGN.md §3.3).
    let code = rest[0];
    let kind = RecordKind::from_code(code).ok_or(FormatError::InvalidRecordKind { code })?;
    let capture_ns = u64::from_le_bytes(arr(rest, 1));
    let len = u32::from_le_bytes(arr(rest, 9)) as usize;
    if len > MAX_RECORD_BYTES {
        return Err(FormatError::RecordTooLarge { len });
    }
    let end = RECORD_HEADER_LEN + len;
    if rest.len() < end {
        return Err(FormatError::RecordTruncated { offset: pos });
    }
    Ok((
        RecordRef {
            kind,
            capture_ns,
            bytes: &rest[RECORD_HEADER_LEN..end],
        },
        pos + end,
    ))
}

/// Validates a decoded chunk body against its header: exact length, exact record count, every capture
/// time inside `[min, max]`, clean framing. Returns the number of records.
pub fn validate_chunk_records(header: &ChunkHeader, raw: &[u8]) -> Result<u32, FormatError> {
    if raw.len() != header.raw_len as usize {
        return Err(FormatError::RawLengthMismatch {
            expected: header.raw_len,
            actual: raw.len(),
        });
    }
    let mut count: u32 = 0;
    for rec in RecordIter::new(raw) {
        let rec = rec?;
        if rec.capture_ns < header.min_capture_ns || rec.capture_ns > header.max_capture_ns {
            return Err(FormatError::RecordOutsideChunkRange {
                capture_ns: rec.capture_ns,
                min_ns: header.min_capture_ns,
                max_ns: header.max_capture_ns,
            });
        }
        count = count.saturating_add(1);
    }
    if count != header.record_count {
        return Err(FormatError::RecordCountMismatch {
            expected: header.record_count,
            actual: count,
        });
    }
    Ok(count)
}

/// Why records are missing from a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GapReason {
    /// The recorder input queue was full (producer faster than the writer).
    Overrun,
    /// A write/sync/create failed; records were dropped until a new segment could be opened.
    WriteFailure,
    /// Free disk space fell below the critical threshold; recording was suspended.
    DiskSpaceCritical,
}

impl GapReason {
    const fn code(self) -> u8 {
        match self {
            GapReason::Overrun => 1,
            GapReason::WriteFailure => 2,
            GapReason::DiskSpaceCritical => 3,
        }
    }

    const fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            1 => GapReason::Overrun,
            2 => GapReason::WriteFailure,
            3 => GapReason::DiskSpaceCritical,
            _ => return None,
        })
    }

    /// Stable upper-case name used in the index and logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            GapReason::Overrun => "OVERRUN",
            GapReason::WriteFailure => "WRITE_FAILURE",
            GapReason::DiskSpaceCritical => "DISK_SPACE_CRITICAL",
        }
    }
}

/// Payload of a [`RecordKind::RecorderNote`] record describing records the recorder had to drop, so a
/// gap in the recording is explicit in the file itself (never silent).
///
/// Layout (28 bytes): `note_type u8 (=1 GAP) | reason u8 | flags u8 (bit0 first present, bit1 last
/// present) | reserved u8 | dropped_records u64 | first_capture_ns u64 | last_capture_ns u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapNote {
    /// Why the records were dropped.
    pub reason: GapReason,
    /// Producer records dropped in this gap.
    pub dropped_records: u64,
    /// Capture time of the earliest dropped record, if known.
    pub first_capture_ns: Option<u64>,
    /// Capture time of the latest dropped record, if known.
    pub last_capture_ns: Option<u64>,
}

/// Encoded length of a [`GapNote`].
pub const GAP_NOTE_LEN: usize = 28;
const NOTE_TYPE_GAP: u8 = 1;
const GAP_FLAG_FIRST: u8 = 0b01;
const GAP_FLAG_LAST: u8 = 0b10;

impl GapNote {
    /// Serializes the note.
    pub fn encode(&self) -> [u8; GAP_NOTE_LEN] {
        let mut b = [0u8; GAP_NOTE_LEN];
        b[0] = NOTE_TYPE_GAP;
        b[1] = self.reason.code();
        let mut flags = 0;
        if self.first_capture_ns.is_some() {
            flags |= GAP_FLAG_FIRST;
        }
        if self.last_capture_ns.is_some() {
            flags |= GAP_FLAG_LAST;
        }
        b[2] = flags;
        b[4..12].copy_from_slice(&self.dropped_records.to_le_bytes());
        b[12..20].copy_from_slice(&self.first_capture_ns.unwrap_or(0).to_le_bytes());
        b[20..28].copy_from_slice(&self.last_capture_ns.unwrap_or(0).to_le_bytes());
        b
    }

    /// Parses a note produced by [`GapNote::encode`].
    pub fn decode(b: &[u8]) -> Result<Self, FormatError> {
        if b.len() != GAP_NOTE_LEN {
            return Err(FormatError::BadGapNote {
                reason: "wrong length",
            });
        }
        if b[0] != NOTE_TYPE_GAP {
            return Err(FormatError::BadGapNote {
                reason: "unknown note type",
            });
        }
        let reason = GapReason::from_code(b[1]).ok_or(FormatError::BadGapNote {
            reason: "unknown reason",
        })?;
        let flags = b[2];
        if flags & !(GAP_FLAG_FIRST | GAP_FLAG_LAST) != 0 || b[3] != 0 {
            return Err(FormatError::BadGapNote {
                reason: "reserved bits set",
            });
        }
        let first = u64::from_le_bytes(arr(b, 12));
        let last = u64::from_le_bytes(arr(b, 20));
        Ok(Self {
            reason,
            dropped_records: u64::from_le_bytes(arr(b, 4)),
            first_capture_ns: (flags & GAP_FLAG_FIRST != 0).then_some(first),
            last_capture_ns: (flags & GAP_FLAG_LAST != 0).then_some(last),
        })
    }
}

/// Copies `N` bytes starting at `at`. Callers guarantee `at + N <= b.len()` (all call sites use fixed
/// offsets into buffers whose length was checked first).
fn arr<const N: usize>(b: &[u8], at: usize) -> [u8; N] {
    let mut a = [0u8; N];
    a.copy_from_slice(&b[at..at + N]);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> FileHeader {
        FileHeader {
            format_version: FORMAT_VERSION,
            telemetry_schema_version: 1,
            session_id: Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            segment_index: 3,
            created_unix_ns: 1_790_000_000_000_000_000,
            created_mono_ns: 42,
            app_version: "0.1.0".into(),
        }
    }

    fn chunk_header() -> ChunkHeader {
        ChunkHeader {
            body_len: 10,
            record_count: 2,
            chunk_seq: 7,
            min_capture_ns: 5,
            max_capture_ns: 9,
            raw_len: 100,
            body_crc32c: 0xdead_beef,
            codec: ChunkCodec::Zstd,
        }
    }

    #[test]
    fn file_header_roundtrip_and_exact_offsets() {
        let h = header();
        let b = h.encode().expect("encode");
        assert_eq!(&b[0..8], &FILE_MAGIC);
        assert_eq!(u16::from_le_bytes([b[8], b[9]]), FORMAT_VERSION);
        assert_eq!(u16::from_le_bytes([b[10], b[11]]), 96);
        assert_eq!(u16::from_le_bytes([b[12], b[13]]), 1);
        assert_eq!(&b[16..32], h.session_id.as_bytes());
        assert_eq!(u32::from_le_bytes([b[32], b[33], b[34], b[35]]), 3);
        assert_eq!(
            i64::from_le_bytes(b[40..48].try_into().expect("8 bytes")),
            h.created_unix_ns
        );
        assert_eq!(
            u64::from_le_bytes(b[48..56].try_into().expect("8 bytes")),
            42
        );
        assert_eq!(b[56], 5);
        assert_eq!(&b[57..62], b"0.1.0");
        assert_eq!(
            u32::from_le_bytes(b[92..96].try_into().expect("4 bytes")),
            crc32c::crc32c(&b[..92])
        );
        assert_eq!(FileHeader::decode(&b).expect("decode"), h);
        assert_eq!(h.created_mono_ns(), Some(42));
    }

    #[test]
    fn file_header_rejects_corruption_everywhere() {
        let b = header().encode().expect("encode");
        for i in 0..FILE_HEADER_LEN {
            let mut c = b;
            c[i] ^= 0x01;
            assert!(
                FileHeader::decode(&c).is_err(),
                "bit flip at byte {i} must be detected"
            );
        }
        for len in 0..FILE_HEADER_LEN {
            assert!(FileHeader::decode(&b[..len]).is_err());
        }
    }

    #[test]
    fn newer_format_version_is_reported_as_unsupported() {
        let mut b = header().encode().expect("encode");
        b[FH_FORMAT_VERSION..FH_FORMAT_VERSION + 2].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            FileHeader::decode(&b),
            Err(FormatError::UnsupportedVersion { found: 2 })
        );
    }

    #[test]
    fn app_version_too_long_is_rejected() {
        let mut h = header();
        h.app_version = "x".repeat(MAX_APP_VERSION_BYTES + 1);
        assert_eq!(
            h.encode(),
            Err(FormatError::AppVersionTooLong {
                len: MAX_APP_VERSION_BYTES + 1
            })
        );
    }

    #[test]
    fn chunk_header_roundtrip_and_exact_offsets() {
        let h = chunk_header();
        let b = h.encode();
        assert_eq!(&b[0..4], b"LRCH");
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().expect("4")), 10);
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().expect("4")), 2);
        assert_eq!(u32::from_le_bytes(b[12..16].try_into().expect("4")), 7);
        assert_eq!(u64::from_le_bytes(b[16..24].try_into().expect("8")), 5);
        assert_eq!(u64::from_le_bytes(b[24..32].try_into().expect("8")), 9);
        assert_eq!(u32::from_le_bytes(b[32..36].try_into().expect("4")), 100);
        assert_eq!(
            u32::from_le_bytes(b[36..40].try_into().expect("4")),
            0xdead_beef
        );
        assert_eq!(b[40], 1);
        assert_eq!(&b[41..44], &[0, 0, 0]);
        assert_eq!(
            u32::from_le_bytes(b[44..48].try_into().expect("4")),
            crc32c::crc32c(&b[..44])
        );
        assert_eq!(ChunkHeader::decode(&b).expect("decode"), h);
        assert_eq!(h.total_len(), 58);
    }

    #[test]
    fn chunk_header_detects_every_bit_flip() {
        let b = chunk_header().encode();
        for i in 0..CHUNK_HEADER_LEN {
            for bit in 0..8 {
                let mut c = b;
                c[i] ^= 1 << bit;
                assert!(
                    ChunkHeader::decode(&c).is_err(),
                    "flip of bit {bit} in byte {i} must be detected"
                );
            }
        }
    }

    #[test]
    fn chunk_header_sanity_checks_run_after_crc() {
        let mut bad = chunk_header();
        bad.min_capture_ns = 10;
        assert!(matches!(
            ChunkHeader::decode(&bad.encode()),
            Err(FormatError::ChunkHeaderInconsistent { .. })
        ));
        let mut bad = chunk_header();
        bad.record_count = 0;
        assert!(ChunkHeader::decode(&bad.encode()).is_err());
        let mut bad = chunk_header();
        bad.record_count = 8; // 8 * 13 > 100
        assert!(ChunkHeader::decode(&bad.encode()).is_err());
        let mut bad = chunk_header();
        bad.raw_len = MAX_CHUNK_RAW_BYTES as u32 + 1;
        assert!(ChunkHeader::decode(&bad.encode()).is_err());
        let mut bad = chunk_header();
        bad.codec = ChunkCodec::Stored; // stored requires body_len == raw_len
        assert!(ChunkHeader::decode(&bad.encode()).is_err());
    }

    #[test]
    fn body_verification() {
        let body = b"0123456789";
        let mut h = chunk_header();
        h.body_crc32c = crc32c::crc32c(body);
        assert_eq!(h.verify_body(body), Ok(()));
        assert!(matches!(
            h.verify_body(b"0123456788"),
            Err(FormatError::ChunkBodyCrc { .. })
        ));
        assert!(matches!(
            h.verify_body(b"012345678"),
            Err(FormatError::BodyLengthMismatch { .. })
        ));
    }

    #[test]
    fn body_bound_covers_zstd_worst_case() {
        let bound = zstd::zstd_safe::compress_bound(MAX_CHUNK_RAW_BYTES);
        assert!(
            MAX_CHUNK_BODY_BYTES >= bound,
            "{MAX_CHUNK_BODY_BYTES} < {bound}"
        );
    }

    #[test]
    fn record_framing_roundtrip_and_truncation() {
        let mut buf = Vec::new();
        encode_record(&mut buf, RecordKind::TelemetryTlv, 1, b"abc").expect("enc");
        encode_record(&mut buf, RecordKind::Unknown(200), 2, b"").expect("enc");
        let recs: Vec<_> = RecordIter::new(&buf)
            .collect::<Result<_, _>>()
            .expect("iter");
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].bytes, b"abc");
        assert_eq!(recs[0].capture_ns, 1);
        assert_eq!(recs[1].kind, RecordKind::Unknown(200));
        let first_len = RECORD_HEADER_LEN + 3;
        for len in 1..buf.len() {
            let items: Vec<_> = RecordIter::new(&buf[..len]).collect();
            assert!(
                items.iter().any(Result::is_err) || len == first_len,
                "len {len}"
            );
        }
        assert_eq!(
            encode_record(&mut buf, RecordKind::Unknown(0), 0, b""),
            Err(FormatError::InvalidRecordKind { code: 0 })
        );
    }

    #[test]
    fn kind_codes_roundtrip() {
        for code in 1..=u8::MAX {
            let k = RecordKind::from_code(code).expect("non-zero code");
            assert_eq!(k.code(), code);
        }
        assert_eq!(RecordKind::from_code(0), None);
        assert!(!RecordKind::RecorderNote.is_producer_kind());
        assert!(!RecordKind::Unknown(99).is_producer_kind());
        assert!(RecordKind::TelemetryTlv.is_producer_kind());
        assert_eq!(
            ChunkCodec::from_code(ChunkCodec::Zstd.code()),
            Some(ChunkCodec::Zstd)
        );
        assert_eq!(ChunkCodec::from_code(2), None);
    }

    #[test]
    fn gap_note_roundtrip() {
        for note in [
            GapNote {
                reason: GapReason::Overrun,
                dropped_records: 7,
                first_capture_ns: Some(1),
                last_capture_ns: Some(9),
            },
            GapNote {
                reason: GapReason::WriteFailure,
                dropped_records: 0,
                first_capture_ns: None,
                last_capture_ns: None,
            },
            GapNote {
                reason: GapReason::DiskSpaceCritical,
                dropped_records: u64::MAX,
                first_capture_ns: Some(0),
                last_capture_ns: None,
            },
        ] {
            assert_eq!(GapNote::decode(&note.encode()).expect("decode"), note);
        }
        assert!(GapNote::decode(&[0u8; 3]).is_err());
        let mut bad = GapNote {
            reason: GapReason::Overrun,
            dropped_records: 1,
            first_capture_ns: None,
            last_capture_ns: None,
        }
        .encode();
        bad[3] = 1;
        assert!(GapNote::decode(&bad).is_err());
    }
}

//! `UiBatch` v1 envelope: the binary batch the desktop bridge sends to the WebView (D-023).
//!
//! ```text
//! 0   4  magic "LRUB"
//! 4   2  version = 1
//! 6   2  flags: bit0 RESET, bit1 BACKFILL, bit2 GAP_BEFORE, other bits must be 0
//! 8   4  batch_seq (u32, +1 per batch per subscription; a jump is treated as GAP_BEFORE)
//! 12  2  block_count
//! 14  2  reserved = 0
//! 16  …  block_count × { u32 len | len bytes = one telemetry-model TLV block }
//! ```
//!
//! All integers are little-endian. [`encode`] is the only producer used by the Rust side (the app-core
//! bridge and the `gen-ts` golden vectors); everything it returns is accepted by [`parse`].
//!
//! [`parse`] reads the envelope field by field in this order and stops at the first problem, like
//! [`crate::codec::decode`] does for TLV blocks; `gen-ts` writes its results to the golden manifest as the
//! outcomes the TypeScript parser must reproduce. It only checks the envelope: block contents are TLV
//! blocks, checked separately by [`crate::codec::decode`].

use thiserror::Error;

/// `"LRUB"`.
pub const MAGIC: [u8; 4] = *b"LRUB";
/// Envelope version this parser accepts.
pub const VERSION: u16 = 1;
/// Discard cached values and ring contents, then apply.
pub const FLAG_RESET: u16 = 1 << 0;
/// Historical blocks that prefill the ring, oldest first.
pub const FLAG_BACKFILL: u16 = 1 << 1;
/// Blocks were dropped before this batch.
pub const FLAG_GAP_BEFORE: u16 = 1 << 2;
/// Every defined flag bit.
pub const KNOWN_FLAGS: u16 = FLAG_RESET | FLAG_BACKFILL | FLAG_GAP_BEFORE;
/// Fixed envelope header size.
pub const HEADER_BYTES: usize = 16;
/// Size of the length prefix in front of every block.
pub const BLOCK_LEN_BYTES: usize = 4;
/// Largest batch (backfills are split across batches).
pub const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;

/// A structurally valid envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBatch {
    /// Envelope version (always [`VERSION`] for an accepted batch).
    pub version: u16,
    /// Flag bits; only [`KNOWN_FLAGS`] bits can be set.
    pub flags: u16,
    /// Per-subscription batch sequence number.
    pub batch_seq: u32,
    /// `(offset, length)` of each block inside the batch.
    pub blocks: Vec<(usize, usize)>,
}

impl ParsedBatch {
    /// The block bytes inside `data`, in batch order. `data` must be the slice this batch was parsed from;
    /// a block range that does not fit `data` yields an empty slice instead of panicking.
    pub fn block_slices<'a>(&'a self, data: &'a [u8]) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.blocks.iter().map(move |&(offset, len)| {
            offset
                .checked_add(len)
                .and_then(|end| data.get(offset..end))
                .unwrap_or(&[])
        })
    }
}

/// Envelope errors. Any of them rejects the whole batch.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UiBatchError {
    /// The batch is longer than [`MAX_BATCH_BYTES`].
    #[error("UiBatch is {length} bytes, maximum is {max}")]
    TooLarge {
        /// Batch length.
        length: usize,
        /// Limit ([`MAX_BATCH_BYTES`]).
        max: usize,
    },
    /// The batch ends inside a field or block.
    #[error("UiBatch truncated: needed {needed} more bytes at offset {offset}")]
    Truncated {
        /// Offset of the field that does not fit.
        offset: usize,
        /// Missing byte count.
        needed: usize,
    },
    /// The first four bytes are not [`MAGIC`].
    #[error("UiBatch magic is {found:?}, expected \"LRUB\"")]
    BadMagic {
        /// The bytes found.
        found: [u8; 4],
    },
    /// The envelope version is not [`VERSION`].
    #[error("UiBatch version {version} is not supported")]
    UnsupportedVersion {
        /// The version found.
        version: u16,
    },
    /// Flag bits outside [`KNOWN_FLAGS`] are set.
    #[error("UiBatch has unknown flag bits {bits:#06x}")]
    UnknownFlags {
        /// The unknown bits.
        bits: u16,
    },
    /// The reserved header field is not zero.
    #[error("UiBatch reserved field is {value}, must be 0")]
    ReservedNonZero {
        /// The value found.
        value: u16,
    },
    /// Bytes follow the last announced block.
    #[error("{count} trailing bytes after the last UiBatch block")]
    TrailingBytes {
        /// Number of extra bytes.
        count: usize,
    },
}

impl UiBatchError {
    /// Variant name, as written to the golden manifest's `kind` field.
    pub fn kind(&self) -> &'static str {
        match self {
            UiBatchError::TooLarge { .. } => "TooLarge",
            UiBatchError::Truncated { .. } => "Truncated",
            UiBatchError::BadMagic { .. } => "BadMagic",
            UiBatchError::UnsupportedVersion { .. } => "UnsupportedVersion",
            UiBatchError::UnknownFlags { .. } => "UnknownFlags",
            UiBatchError::ReservedNonZero { .. } => "ReservedNonZero",
            UiBatchError::TrailingBytes { .. } => "TrailingBytes",
        }
    }
}

/// Reasons [`encode`] refuses to build a batch. Each one would otherwise produce a batch [`parse`] rejects.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UiBatchEncodeError {
    /// Flag bits outside [`KNOWN_FLAGS`] were requested.
    #[error("unknown UiBatch flag bits {bits:#06x}")]
    UnknownFlags {
        /// The unknown bits.
        bits: u16,
    },
    /// More blocks than the u16 `block_count` field can announce.
    #[error("{count} blocks do not fit one UiBatch (maximum {max})", max = u16::MAX)]
    TooManyBlocks {
        /// Requested block count.
        count: usize,
    },
    /// The encoded batch would exceed [`MAX_BATCH_BYTES`]; split it across batches.
    #[error("UiBatch would be {length} bytes, maximum is {max}")]
    TooLarge {
        /// Encoded length (saturated at `usize::MAX`).
        length: usize,
        /// Limit ([`MAX_BATCH_BYTES`]).
        max: usize,
    },
}

/// Encoded size of a batch holding blocks of the given lengths (saturating at `usize::MAX`).
pub fn encoded_len<I>(block_lens: I) -> usize
where
    I: IntoIterator<Item = usize>,
{
    block_lens.into_iter().fold(HEADER_BYTES, |total, len| {
        total.saturating_add(BLOCK_LEN_BYTES).saturating_add(len)
    })
}

/// Encodes a batch. `blocks` are complete TLV blocks, oldest first. The result always passes [`parse`].
pub fn encode(flags: u16, batch_seq: u32, blocks: &[&[u8]]) -> Result<Vec<u8>, UiBatchEncodeError> {
    if flags & !KNOWN_FLAGS != 0 {
        return Err(UiBatchEncodeError::UnknownFlags {
            bits: flags & !KNOWN_FLAGS,
        });
    }
    let count = u16::try_from(blocks.len()).map_err(|_| UiBatchEncodeError::TooManyBlocks {
        count: blocks.len(),
    })?;
    let length = encoded_len(blocks.iter().map(|b| b.len()));
    if length > MAX_BATCH_BYTES {
        return Err(UiBatchEncodeError::TooLarge {
            length,
            max: MAX_BATCH_BYTES,
        });
    }
    let mut out = Vec::with_capacity(length);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&batch_seq.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    for block in blocks {
        // Cannot fail: the whole batch is at most MAX_BATCH_BYTES, far below u32::MAX.
        let len = u32::try_from(block.len()).map_err(|_| UiBatchEncodeError::TooLarge {
            length,
            max: MAX_BATCH_BYTES,
        })?;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(block);
    }
    Ok(out)
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], UiBatchError> {
        let end = self.pos.checked_add(n).ok_or(UiBatchError::Truncated {
            offset: self.pos,
            needed: n,
        })?;
        if end > self.data.len() {
            return Err(UiBatchError::Truncated {
                offset: self.pos,
                needed: end - self.data.len(),
            });
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], UiBatchError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
}

/// Parses and validates the envelope.
pub fn parse(data: &[u8]) -> Result<ParsedBatch, UiBatchError> {
    if data.len() > MAX_BATCH_BYTES {
        return Err(UiBatchError::TooLarge {
            length: data.len(),
            max: MAX_BATCH_BYTES,
        });
    }
    let mut c = Cursor { data, pos: 0 };
    let magic: [u8; 4] = c.array()?;
    if magic != MAGIC {
        return Err(UiBatchError::BadMagic { found: magic });
    }
    let version = u16::from_le_bytes(c.array()?);
    if version != VERSION {
        return Err(UiBatchError::UnsupportedVersion { version });
    }
    let flags = u16::from_le_bytes(c.array()?);
    if flags & !KNOWN_FLAGS != 0 {
        return Err(UiBatchError::UnknownFlags {
            bits: flags & !KNOWN_FLAGS,
        });
    }
    let batch_seq = u32::from_le_bytes(c.array()?);
    let count = u16::from_le_bytes(c.array()?);
    let reserved = u16::from_le_bytes(c.array()?);
    if reserved != 0 {
        return Err(UiBatchError::ReservedNonZero { value: reserved });
    }
    let mut blocks = Vec::new();
    for _ in 0..count {
        let len = u32::from_le_bytes(c.array()?) as usize;
        let offset = c.pos;
        c.take(len)?;
        blocks.push((offset, len));
    }
    if c.pos != data.len() {
        return Err(UiBatchError::TrailingBytes {
            count: data.len() - c.pos,
        });
    }
    Ok(ParsedBatch {
        version,
        flags,
        batch_seq,
        blocks,
    })
}

/// Gap detection across consecutive batches of one subscription: a batch follows a gap if it carries
/// GAP_BEFORE, or if its `batch_seq` is not the previous one + 1 (wrapping). A RESET batch starts a new
/// sequence, so it never counts as a jump.
#[derive(Debug, Default)]
pub struct SequenceState {
    last: Option<u32>,
}

impl SequenceState {
    /// Observes an accepted batch; returns true if a gap precedes it.
    pub fn observe(&mut self, flags: u16, batch_seq: u32) -> bool {
        let reset = flags & FLAG_RESET != 0;
        let jump = !reset
            && self
                .last
                .is_some_and(|last| batch_seq != last.wrapping_add(1));
        self.last = Some(batch_seq);
        flags & FLAG_GAP_BEFORE != 0 || jump
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{self, RELAY_NS_UNKNOWN, TlvEncoder, TlvHeader};
    use crate::{ChannelId, Sample, TELEMETRY_SCHEMA_VERSION, Value};

    #[test]
    fn roundtrip_and_structural_errors() {
        let blocks: [&[u8]; 2] = [&[1, 2, 3], &[]];
        let batch = encode(FLAG_RESET | FLAG_BACKFILL, 7, &blocks).expect("encode");
        let parsed = parse(&batch).expect("parse");
        assert_eq!(parsed.flags, FLAG_RESET | FLAG_BACKFILL);
        assert_eq!(parsed.batch_seq, 7);
        assert_eq!(parsed.blocks, vec![(20, 3), (27, 0)]);
        for len in 0..batch.len() {
            assert!(parse(&batch[..len]).is_err(), "prefix {len} must not parse");
        }
        let mut extra = batch.clone();
        extra.push(0);
        assert_eq!(parse(&extra), Err(UiBatchError::TrailingBytes { count: 1 }));
        let too_large = vec![0u8; MAX_BATCH_BYTES + 1];
        assert_eq!(parse(&too_large).map_err(|e| e.kind()), Err("TooLarge"));
    }

    #[test]
    fn sequence_jumps_and_resets() {
        let mut s = SequenceState::default();
        assert!(!s.observe(FLAG_RESET, 0));
        assert!(!s.observe(0, 1));
        assert!(s.observe(0, 3), "2 is missing");
        assert!(s.observe(FLAG_GAP_BEFORE, 4), "explicit gap");
        assert!(!s.observe(FLAG_RESET, 100), "a reset restarts the sequence");
        let mut w = SequenceState::default();
        assert!(!w.observe(0, u32::MAX));
        assert!(!w.observe(0, 0), "wrap-around is not a jump");
    }

    #[test]
    fn encode_refuses_what_parse_would_reject() {
        assert_eq!(
            encode(1 << 3, 0, &[]),
            Err(UiBatchEncodeError::UnknownFlags { bits: 1 << 3 })
        );
        let empty: &[u8] = &[];
        let many = vec![empty; usize::from(u16::MAX) + 1];
        assert_eq!(
            encode(0, 0, &many),
            Err(UiBatchEncodeError::TooManyBlocks { count: many.len() })
        );
        // Largest single block that still fits, then one byte more.
        let fits = vec![0u8; MAX_BATCH_BYTES - HEADER_BYTES - BLOCK_LEN_BYTES];
        let at_limit = encode(0, 0, &[&fits]).expect("batch of exactly MAX_BATCH_BYTES");
        assert_eq!(at_limit.len(), MAX_BATCH_BYTES);
        assert!(parse(&at_limit).is_ok());
        let over = vec![0u8; fits.len() + 1];
        assert_eq!(
            encode(0, 0, &[&over]),
            Err(UiBatchEncodeError::TooLarge {
                length: MAX_BATCH_BYTES + 1,
                max: MAX_BATCH_BYTES
            })
        );
        let max_blocks = vec![empty; usize::from(u16::MAX)];
        let batch = encode(KNOWN_FLAGS, u32::MAX, &max_blocks).expect("u16::MAX empty blocks");
        assert_eq!(batch.len(), encoded_len(max_blocks.iter().map(|b| b.len())));
        assert_eq!(parse(&batch).expect("parse").blocks.len(), max_blocks.len());
    }

    fn tlv_block(et: f64, speed: f32) -> Vec<u8> {
        let header = TlvHeader {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            capture_mono_ns: 1_000_000_000 + (et * 1.0e9) as u64,
            capture_relay_ns_est: RELAY_NS_UNKNOWN,
            source_et_s: et,
        };
        let mut buf = Vec::new();
        let mut enc = TlvEncoder::begin(&mut buf, &header);
        enc.push(ChannelId(1), &Sample::valid(Value::F32(speed)))
            .expect("push speed");
        enc.push(
            ChannelId(2),
            &Sample::valid(Value::Str("Porsche 963".into())),
        )
        .expect("push string");
        enc.push(ChannelId(3), &Sample::unavailable())
            .expect("push unavailable");
        enc.finish();
        buf
    }

    #[test]
    fn roundtrip_with_tlv_encoder() {
        let blocks: Vec<Vec<u8>> = (0..5)
            .map(|i| tlv_block(100.0 + f64::from(i) * 0.01, 50.0 + i as f32))
            .collect();
        let refs: Vec<&[u8]> = blocks.iter().map(Vec::as_slice).collect();
        let batch = encode(FLAG_RESET | FLAG_BACKFILL, 0, &refs).expect("encode");
        assert_eq!(batch.len(), encoded_len(blocks.iter().map(Vec::len)));
        let parsed = parse(&batch).expect("parse");
        assert_eq!(parsed.version, VERSION);
        assert_eq!(parsed.flags, FLAG_RESET | FLAG_BACKFILL);
        assert_eq!(parsed.batch_seq, 0);
        let slices: Vec<&[u8]> = parsed.block_slices(&batch).collect();
        assert_eq!(slices, refs, "blocks come back byte-for-byte, in order");
        for (slice, original) in slices.iter().zip(&blocks) {
            assert_eq!(
                codec::decode(slice).expect("decode block from batch"),
                codec::decode(original).expect("decode original block")
            );
        }
    }

    #[test]
    fn block_slices_never_panic_on_a_foreign_slice() {
        let batch = encode(0, 1, &[&[1, 2, 3]]).expect("encode");
        let parsed = parse(&batch).expect("parse");
        let short: Vec<&[u8]> = parsed.block_slices(&batch[..4]).collect();
        assert_eq!(short, vec![&[] as &[u8]]);
    }

    #[test]
    fn errors_display() {
        let e = UiBatchError::Truncated {
            offset: 4,
            needed: 2,
        };
        assert_eq!(
            e.to_string(),
            "UiBatch truncated: needed 2 more bytes at offset 4"
        );
        assert_eq!(e.kind(), "Truncated");
        assert!(
            UiBatchEncodeError::TooManyBlocks { count: 70_000 }
                .to_string()
                .contains("65535")
        );
    }
}

//! Reference encoder and parser for the `UiBatch` v1 envelope (D-023).
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
//! The parser reads the envelope field by field in this order and stops at the first problem, like
//! `telemetry_model::codec::decode` does for TLV blocks; its results are the expected outcomes written to
//! the golden manifest. It only checks the envelope: block contents are TLV blocks, checked separately.

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
/// Largest batch (backfills are split across batches).
pub const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;

/// A structurally valid envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBatch {
    pub version: u16,
    pub flags: u16,
    pub batch_seq: u32,
    /// `(offset, length)` of each block inside the batch.
    pub blocks: Vec<(usize, usize)>,
}

/// Envelope errors. Any of them rejects the whole batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiBatchError {
    TooLarge { length: usize, max: usize },
    Truncated { offset: usize, needed: usize },
    BadMagic { found: [u8; 4] },
    UnsupportedVersion { version: u16 },
    UnknownFlags { bits: u16 },
    ReservedNonZero { value: u16 },
    TrailingBytes { count: usize },
}

impl UiBatchError {
    /// Variant name, as written to the manifest's `kind` field.
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

/// Encodes a batch. `blocks` are complete TLV blocks.
pub fn encode(flags: u16, batch_seq: u32, blocks: &[&[u8]]) -> Vec<u8> {
    let count = u16::try_from(blocks.len()).unwrap_or_else(|_| panic!("too many blocks"));
    let mut out =
        Vec::with_capacity(HEADER_BYTES + blocks.iter().map(|b| 4 + b.len()).sum::<usize>());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&batch_seq.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    for block in blocks {
        let len = u32::try_from(block.len()).unwrap_or_else(|_| panic!("block too large"));
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(block);
    }
    out
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

    #[test]
    fn roundtrip_and_structural_errors() {
        let blocks: [&[u8]; 2] = [&[1, 2, 3], &[]];
        let batch = encode(FLAG_RESET | FLAG_BACKFILL, 7, &blocks);
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
}

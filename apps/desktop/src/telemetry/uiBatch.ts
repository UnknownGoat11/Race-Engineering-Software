/**
 * Strict parser for the `UiBatch` v1 envelope that carries telemetry from Rust to the WebView (D-023).
 *
 * ```text
 * 0   4  magic "LRUB"
 * 4   2  version = 1
 * 6   2  flags: bit0 RESET, bit1 BACKFILL, bit2 GAP_BEFORE; other bits must be 0
 * 8   4  batch_seq (u32, +1 per batch per subscription; a jump is treated as GAP_BEFORE)
 * 12  2  block_count
 * 14  2  reserved = 0
 * 16  …  block_count × { u32 len | len bytes = one TLV block (tlv.ts) }
 * ```
 * All integers are little-endian. Fields are read in this order and the first problem is reported, like the
 * Rust reference parser in tools/gen-ts whose results are the golden expectations. Any error rejects the whole
 * batch. The parser checks only the envelope; the blocks are TLV blocks, decoded separately.
 */
import { err, ok, type Result } from './result';

/** The four magic bytes, `"LRUB"`. */
export const UI_BATCH_MAGIC: readonly number[] = [0x4c, 0x52, 0x55, 0x42];
/** Envelope version this parser accepts. */
export const UI_BATCH_VERSION = 1;
/** Size of the fixed envelope header. */
export const UI_BATCH_HEADER_BYTES = 16;
/** Discard cached values and ring contents, then apply. */
export const UI_BATCH_FLAG_RESET = 1 << 0;
/** Historical blocks that prefill the ring, oldest first. */
export const UI_BATCH_FLAG_BACKFILL = 1 << 1;
/** Blocks were dropped before this batch. */
export const UI_BATCH_FLAG_GAP_BEFORE = 1 << 2;
/** Every defined flag bit; any other bit is an error. */
export const UI_BATCH_KNOWN_FLAGS = UI_BATCH_FLAG_RESET | UI_BATCH_FLAG_BACKFILL | UI_BATCH_FLAG_GAP_BEFORE;
/** Largest accepted batch (4 MiB; backfills are split across batches). */
export const UI_BATCH_MAX_BYTES = 4 * 1024 * 1024;

/** Byte range of one block inside the batch. */
export interface UiBatchBlock {
  /** Offset of the block's first byte, relative to the start of the batch. */
  readonly offset: number;
  readonly length: number;
}

/** A structurally valid batch. */
export interface UiBatch {
  readonly version: number;
  readonly flags: number;
  readonly reset: boolean;
  readonly backfill: boolean;
  readonly gapBefore: boolean;
  /** u32 sequence number. */
  readonly batchSeq: number;
  readonly blocks: readonly UiBatchBlock[];
}

/** Envelope errors; offsets are relative to the start of the batch. */
export type UiBatchError =
  | { readonly kind: 'TooLarge'; readonly length: number; readonly max: number }
  | { readonly kind: 'Truncated'; readonly offset: number; readonly needed: number }
  /** `found` is the four magic bytes as lower-case hex. */
  | { readonly kind: 'BadMagic'; readonly found: string }
  | { readonly kind: 'UnsupportedVersion'; readonly version: number }
  /** `bits` are the undefined flag bits that were set. */
  | { readonly kind: 'UnknownFlags'; readonly bits: number }
  | { readonly kind: 'ReservedNonZero'; readonly value: number }
  | { readonly kind: 'TrailingBytes'; readonly count: number };

const MAGIC_BYTES = 4;
const U16_BYTES = 2;
const U32_BYTES = 4;
const HEX_RADIX = 16;
const HEX_BYTE_DIGITS = 2;

function truncated(offset: number, needed: number, length: number): UiBatchError {
  return { kind: 'Truncated', offset, needed: offset + needed - length };
}

function hex(bytes: Uint8Array): string {
  let out = '';
  for (const b of bytes) {
    out += b.toString(HEX_RADIX).padStart(HEX_BYTE_DIGITS, '0');
  }
  return out;
}

/** Parses and validates a batch envelope. `bytes` may be a view into a larger buffer. */
export function parseUiBatch(bytes: Uint8Array): Result<UiBatch, UiBatchError> {
  const length = bytes.byteLength;
  if (length > UI_BATCH_MAX_BYTES) return err({ kind: 'TooLarge', length, max: UI_BATCH_MAX_BYTES });
  const view = new DataView(bytes.buffer, bytes.byteOffset, length);
  let pos = 0;

  if (pos + MAGIC_BYTES > length) return err(truncated(pos, MAGIC_BYTES, length));
  const magic = bytes.subarray(pos, pos + MAGIC_BYTES);
  if (!UI_BATCH_MAGIC.every((b, i) => magic[i] === b)) return err({ kind: 'BadMagic', found: hex(magic) });
  pos += MAGIC_BYTES;

  if (pos + U16_BYTES > length) return err(truncated(pos, U16_BYTES, length));
  const version = view.getUint16(pos, true);
  if (version !== UI_BATCH_VERSION) return err({ kind: 'UnsupportedVersion', version });
  pos += U16_BYTES;

  if (pos + U16_BYTES > length) return err(truncated(pos, U16_BYTES, length));
  const flags = view.getUint16(pos, true);
  const unknownBits = flags & ~UI_BATCH_KNOWN_FLAGS;
  if (unknownBits !== 0) return err({ kind: 'UnknownFlags', bits: unknownBits });
  pos += U16_BYTES;

  if (pos + U32_BYTES > length) return err(truncated(pos, U32_BYTES, length));
  const batchSeq = view.getUint32(pos, true);
  pos += U32_BYTES;

  if (pos + U16_BYTES > length) return err(truncated(pos, U16_BYTES, length));
  const blockCount = view.getUint16(pos, true);
  pos += U16_BYTES;

  if (pos + U16_BYTES > length) return err(truncated(pos, U16_BYTES, length));
  const reserved = view.getUint16(pos, true);
  if (reserved !== 0) return err({ kind: 'ReservedNonZero', value: reserved });
  pos += U16_BYTES;

  // Blocks are appended as they are found: an untrusted count never sizes an allocation.
  const blocks: UiBatchBlock[] = [];
  for (let i = 0; i < blockCount; i++) {
    if (pos + U32_BYTES > length) return err(truncated(pos, U32_BYTES, length));
    const blockLength = view.getUint32(pos, true);
    pos += U32_BYTES;
    if (pos + blockLength > length) return err(truncated(pos, blockLength, length));
    blocks.push({ offset: pos, length: blockLength });
    pos += blockLength;
  }
  if (pos !== length) return err({ kind: 'TrailingBytes', count: length - pos });

  return ok({
    version,
    flags,
    reset: (flags & UI_BATCH_FLAG_RESET) !== 0,
    backfill: (flags & UI_BATCH_FLAG_BACKFILL) !== 0,
    gapBefore: (flags & UI_BATCH_FLAG_GAP_BEFORE) !== 0,
    batchSeq,
    blocks,
  });
}

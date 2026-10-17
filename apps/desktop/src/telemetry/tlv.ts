/**
 * Telemetry TLV block decoder — a TypeScript mirror of `telemetry-model::codec::decode` (docs/PROTOCOL.md §4).
 *
 * Layout (little-endian):
 * ```text
 * u16 schema_version | u64 capture_mono_ns | i64 capture_relay_ns_est | f64 source_et_s | u16 record_count
 * record_count × ( u16 channel_id | u8 tag | value bytes )      tag = (quality << 5) | value_type
 * ```
 * The decoder reads fields in exactly the Rust order and reports the same errors with the same offsets, so the
 * golden vectors produced by the Rust codec (`src/generated/golden`) decode identically here. Every read is
 * bounds-checked before it happens and nothing is allocated from the untrusted record count.
 */
import {
  ALL_QUALITIES,
  ALL_VALUE_TYPES,
  MAX_STR_VALUE_BYTES,
  TLV_HEADER_BYTES,
  ValueType,
  type Quality,
} from '../generated/channels';
import { err, ok, type Result } from './result';

/** Fixed header of a TLV block (`codec::TlvHeader`). */
export interface TlvHeader {
  readonly schemaVersion: number;
  /** Driver monotonic nanoseconds at the moment the LMU data was read (u64). */
  readonly captureMonoNs: bigint;
  /** Driver's estimate of the relay clock at capture (ns since the Unix epoch, i64), or `RELAY_NS_UNKNOWN`. */
  readonly captureRelayNsEst: bigint;
  /** LMU elapsed session time of the sample in seconds; NaN when unavailable. */
  readonly sourceEtS: number;
}

/** A decoded value (`value::Value`). U64 is a bigint so values above 2^53 keep full precision. */
export type TlvValue =
  | { readonly type: typeof ValueType.None }
  | { readonly type: typeof ValueType.F32; readonly value: number }
  | { readonly type: typeof ValueType.F64; readonly value: number }
  | { readonly type: typeof ValueType.I32; readonly value: number }
  | { readonly type: typeof ValueType.U32; readonly value: number }
  | { readonly type: typeof ValueType.Bool; readonly value: boolean }
  | { readonly type: typeof ValueType.U8; readonly value: number }
  | { readonly type: typeof ValueType.I8; readonly value: number }
  | { readonly type: typeof ValueType.Str; readonly value: string }
  | { readonly type: typeof ValueType.U64; readonly value: bigint };

/** One decoded record (`codec::TlvRecord`). The channel id is returned even if this build does not know it. */
export interface TlvRecord {
  readonly channel: number;
  readonly quality: Quality;
  readonly value: TlvValue;
}

/** Decode errors (`codec::CodecError`); offsets are relative to the start of the block. */
export type CodecError =
  | { readonly kind: 'Truncated'; readonly offset: number; readonly needed: number }
  | { readonly kind: 'UnknownValueType'; readonly valueType: number }
  | { readonly kind: 'UnknownQuality'; readonly quality: number }
  | { readonly kind: 'StringTooLong'; readonly length: number }
  | { readonly kind: 'InvalidUtf8' }
  /** Encoder-only in Rust (the wire count is a u16); kept so the error type mirrors `CodecError` fully. */
  | { readonly kind: 'TooManyRecords'; readonly count: number }
  | { readonly kind: 'TrailingBytes'; readonly count: number };

/** A decoded scalar as handed to a visitor: number, boolean (BOOL), string (STR) or bigint (U64). */
export type TlvScalar = number | bigint | boolean | string;

/** Receives the records of a block as they are decoded (allocation-free path used by the store). */
export interface TlvRecordVisitor {
  /** One record; `value` is undefined for type NONE. */
  record(channel: number, quality: Quality, valueType: ValueType, value: TlvScalar | undefined): void;
}

/** Header and record count of a block that decoded completely. */
export interface TlvBlockSummary {
  readonly header: TlvHeader;
  readonly recordCount: number;
}

const QUALITY_SHIFT = 5;
const VALUE_TYPE_MASK = 0b1_1111;
const U16_BYTES = 2;
const U32_BYTES = 4;
const U64_BYTES = 8;
const BYTE = 1;

/** Code → enum value tables built from the generated (Rust-derived) lists, so unknown codes are undefined. */
const VALUE_TYPE_BY_CODE: readonly (ValueType | undefined)[] = codeTable(ALL_VALUE_TYPES);
const QUALITY_BY_CODE: readonly (Quality | undefined)[] = codeTable(ALL_QUALITIES);

function codeTable<T extends number>(codes: readonly T[]): (T | undefined)[] {
  const table: (T | undefined)[] = [];
  for (const code of codes) {
    table[code] = code;
  }
  return table;
}

/**
 * Strict UTF-8 like Rust's `str::from_utf8` (no replacement characters; overlong forms, surrogates and code
 * points above U+10FFFF are errors). `ignoreBOM` keeps a leading U+FEFF, which Rust also keeps.
 */
const UTF8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });

function truncated(offset: number, needed: number, length: number): CodecError {
  return { kind: 'Truncated', offset, needed: offset + needed - length };
}

/**
 * Decodes the TLV block stored at `view[offset, offset + length)` and calls `visitor` for each record, in
 * order. Mirrors `codec::decode` exactly (field order, error kinds, offsets relative to the block start).
 *
 * Records are visited as they are decoded, so when a later record fails the visitor has already seen the
 * earlier ones. Callers that must not act on a failing block (the store never partially applies a batch)
 * scan it with a `null` visitor first.
 */
export function readTlvBlock(
  view: DataView,
  offset: number,
  length: number,
  visitor: TlvRecordVisitor | null,
): Result<TlvBlockSummary, CodecError> {
  let pos = 0;
  // Header, field by field, so a short block reports the field that is cut, as the Rust cursor does.
  if (pos + U16_BYTES > length) return err(truncated(pos, U16_BYTES, length));
  const schemaVersion = view.getUint16(offset + pos, true);
  pos += U16_BYTES;
  if (pos + U64_BYTES > length) return err(truncated(pos, U64_BYTES, length));
  const captureMonoNs = view.getBigUint64(offset + pos, true);
  pos += U64_BYTES;
  if (pos + U64_BYTES > length) return err(truncated(pos, U64_BYTES, length));
  const captureRelayNsEst = view.getBigInt64(offset + pos, true);
  pos += U64_BYTES;
  if (pos + U64_BYTES > length) return err(truncated(pos, U64_BYTES, length));
  const sourceEtS = view.getFloat64(offset + pos, true);
  pos += U64_BYTES;
  if (pos + U16_BYTES > length) return err(truncated(pos, U16_BYTES, length));
  const count = view.getUint16(offset + pos, true);
  pos += U16_BYTES;

  for (let i = 0; i < count; i++) {
    if (pos + U16_BYTES > length) return err(truncated(pos, U16_BYTES, length));
    const channel = view.getUint16(offset + pos, true);
    pos += U16_BYTES;
    if (pos + BYTE > length) return err(truncated(pos, BYTE, length));
    const tag = view.getUint8(offset + pos);
    pos += BYTE;
    // The value type is checked before the quality, as in Rust.
    const valueTypeCode = tag & VALUE_TYPE_MASK;
    const valueType = VALUE_TYPE_BY_CODE[valueTypeCode];
    if (valueType === undefined) return err({ kind: 'UnknownValueType', valueType: valueTypeCode });
    const qualityCode = tag >>> QUALITY_SHIFT;
    const quality = QUALITY_BY_CODE[qualityCode];
    if (quality === undefined) return err({ kind: 'UnknownQuality', quality: qualityCode });

    let value: TlvScalar | undefined;
    switch (valueType) {
      case ValueType.None:
        value = undefined;
        break;
      case ValueType.F32:
        if (pos + U32_BYTES > length) return err(truncated(pos, U32_BYTES, length));
        value = view.getFloat32(offset + pos, true);
        pos += U32_BYTES;
        break;
      case ValueType.F64:
        if (pos + U64_BYTES > length) return err(truncated(pos, U64_BYTES, length));
        value = view.getFloat64(offset + pos, true);
        pos += U64_BYTES;
        break;
      case ValueType.I32:
        if (pos + U32_BYTES > length) return err(truncated(pos, U32_BYTES, length));
        value = view.getInt32(offset + pos, true);
        pos += U32_BYTES;
        break;
      case ValueType.U32:
        if (pos + U32_BYTES > length) return err(truncated(pos, U32_BYTES, length));
        value = view.getUint32(offset + pos, true);
        pos += U32_BYTES;
        break;
      case ValueType.Bool:
        if (pos + BYTE > length) return err(truncated(pos, BYTE, length));
        // Any non-zero byte is true (`c.u8()? != 0`).
        value = view.getUint8(offset + pos) !== 0;
        pos += BYTE;
        break;
      case ValueType.U8:
        if (pos + BYTE > length) return err(truncated(pos, BYTE, length));
        value = view.getUint8(offset + pos);
        pos += BYTE;
        break;
      case ValueType.I8:
        if (pos + BYTE > length) return err(truncated(pos, BYTE, length));
        value = view.getInt8(offset + pos);
        pos += BYTE;
        break;
      case ValueType.Str: {
        if (pos + BYTE > length) return err(truncated(pos, BYTE, length));
        const byteLength = view.getUint8(offset + pos);
        pos += BYTE;
        // The length is checked before the bytes are read, as in Rust.
        if (byteLength > MAX_STR_VALUE_BYTES) return err({ kind: 'StringTooLong', length: byteLength });
        if (pos + byteLength > length) return err(truncated(pos, byteLength, length));
        const bytes = new Uint8Array(view.buffer, view.byteOffset + offset + pos, byteLength);
        try {
          value = UTF8.decode(bytes);
        } catch {
          return err({ kind: 'InvalidUtf8' });
        }
        pos += byteLength;
        break;
      }
      case ValueType.U64:
        if (pos + U64_BYTES > length) return err(truncated(pos, U64_BYTES, length));
        value = view.getBigUint64(offset + pos, true);
        pos += U64_BYTES;
        break;
    }
    visitor?.record(channel, quality, valueType, value);
  }
  if (pos !== length) return err({ kind: 'TrailingBytes', count: length - pos });
  return ok({ header: { schemaVersion, captureMonoNs, captureRelayNsEst, sourceEtS }, recordCount: count });
}

/** A fully decoded block. */
export interface DecodedTlvBlock {
  readonly header: TlvHeader;
  readonly records: readonly TlvRecord[];
}

/** Narrows a visitor scalar to the TypeScript type its value type promises. */
function toTlvValue(valueType: ValueType, value: TlvScalar | undefined): TlvValue {
  switch (valueType) {
    case ValueType.None:
      return { type: valueType };
    case ValueType.Bool:
      if (typeof value === 'boolean') return { type: valueType, value };
      break;
    case ValueType.Str:
      if (typeof value === 'string') return { type: valueType, value };
      break;
    case ValueType.U64:
      if (typeof value === 'bigint') return { type: valueType, value };
      break;
    case ValueType.F32:
    case ValueType.F64:
    case ValueType.I32:
    case ValueType.U32:
    case ValueType.U8:
    case ValueType.I8:
      if (typeof value === 'number') return { type: valueType, value };
      break;
  }
  throw new Error(`TLV decoder produced a ${typeof value} for value type ${String(valueType)}`);
}

/**
 * Decodes a complete TLV block into header and records (`codec::decode`). Unknown channel ids are returned
 * as-is; the caller decides whether to skip them.
 */
export function decodeTlv(bytes: Uint8Array): Result<DecodedTlvBlock, CodecError> {
  const records: TlvRecord[] = [];
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const result = readTlvBlock(view, 0, bytes.byteLength, {
    record(channel, quality, valueType, value) {
      records.push({ channel, quality, value: toTlvValue(valueType, value) });
    },
  });
  if (!result.ok) return result;
  return ok({ header: result.value.header, records });
}

/** Smallest possible block: a header announcing zero records. */
export const MIN_TLV_BLOCK_BYTES = TLV_HEADER_BYTES;

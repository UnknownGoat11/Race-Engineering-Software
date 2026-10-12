// Test support: TLV block and UiBatch encoders for fixtures (tests and benches only, never production code).
// The TLV encoder follows telemetry-model's `TlvEncoder::push` rules (a non-VALID sample is written without a
// value; a VALID sample without a value is written as UNAVAILABLE); tlv.test.ts checks it byte for byte
// against blocks written by the Rust encoder.
import {
  MAX_STR_VALUE_BYTES,
  Quality,
  RELAY_NS_UNKNOWN,
  TELEMETRY_SCHEMA_VERSION,
  TLV_HEADER_BYTES,
  ValueType,
} from '../generated/channels';
import type { TlvValue } from '../telemetry/tlv';
import { UI_BATCH_HEADER_BYTES, UI_BATCH_MAGIC, UI_BATCH_VERSION } from '../telemetry/uiBatch';

/** Block header for a fixture; omitted clocks default to 0 / unknown, the schema to this build's. */
export interface FixtureHeader {
  readonly sourceEtS: number;
  readonly schemaVersion?: number;
  readonly captureMonoNs?: bigint;
  readonly captureRelayNsEst?: bigint;
}

/** One record to encode. */
export interface FixtureRecord {
  readonly channel: number;
  readonly quality: Quality;
  readonly value: TlvValue;
}

const QUALITY_SHIFT = 5;
const RECORD_PREFIX_BYTES = 3;
const NONE: TlvValue = { type: ValueType.None };
const UTF8 = new TextEncoder();

function valueBytes(value: TlvValue): number {
  switch (value.type) {
    case ValueType.None:
      return 0;
    case ValueType.Bool:
    case ValueType.U8:
    case ValueType.I8:
      return 1;
    case ValueType.F32:
    case ValueType.I32:
    case ValueType.U32:
      return 4;
    case ValueType.F64:
    case ValueType.U64:
      return 8;
    case ValueType.Str:
      return 1 + UTF8.encode(value.value).byteLength;
  }
}

/** Encodes one TLV block. */
export function encodeTlvBlock(header: FixtureHeader, records: readonly FixtureRecord[]): Uint8Array {
  const normalized = records.map((r) => {
    const value = r.quality === Quality.Valid ? r.value : NONE;
    const quality = r.quality === Quality.Valid && value.type === ValueType.None ? Quality.Unavailable : r.quality;
    return { channel: r.channel, quality, value };
  });
  const size = normalized.reduce((n, r) => n + RECORD_PREFIX_BYTES + valueBytes(r.value), TLV_HEADER_BYTES);
  const out = new Uint8Array(size);
  const view = new DataView(out.buffer);
  view.setUint16(0, header.schemaVersion ?? TELEMETRY_SCHEMA_VERSION, true);
  view.setBigUint64(2, header.captureMonoNs ?? 0n, true);
  view.setBigInt64(10, header.captureRelayNsEst ?? RELAY_NS_UNKNOWN, true);
  view.setFloat64(18, header.sourceEtS, true);
  view.setUint16(26, normalized.length, true);
  let pos = TLV_HEADER_BYTES;
  for (const r of normalized) {
    view.setUint16(pos, r.channel, true);
    view.setUint8(pos + 2, (r.quality << QUALITY_SHIFT) | r.value.type);
    pos += RECORD_PREFIX_BYTES;
    const v = r.value;
    switch (v.type) {
      case ValueType.None:
        break;
      case ValueType.F32:
        view.setFloat32(pos, v.value, true);
        break;
      case ValueType.F64:
        view.setFloat64(pos, v.value, true);
        break;
      case ValueType.I32:
        view.setInt32(pos, v.value, true);
        break;
      case ValueType.U32:
        view.setUint32(pos, v.value, true);
        break;
      case ValueType.Bool:
        view.setUint8(pos, v.value ? 1 : 0);
        break;
      case ValueType.U8:
        view.setUint8(pos, v.value);
        break;
      case ValueType.I8:
        view.setInt8(pos, v.value);
        break;
      case ValueType.Str: {
        const bytes = UTF8.encode(v.value);
        if (bytes.byteLength > MAX_STR_VALUE_BYTES) throw new Error(`string of ${bytes.byteLength} bytes`);
        view.setUint8(pos, bytes.byteLength);
        out.set(bytes, pos + 1);
        break;
      }
      case ValueType.U64:
        view.setBigUint64(pos, v.value, true);
        break;
    }
    pos += valueBytes(v);
  }
  return out;
}

/** Encodes a UiBatch v1 envelope around complete TLV blocks. */
export function encodeUiBatch(flags: number, batchSeq: number, blocks: readonly Uint8Array[]): Uint8Array {
  const size = blocks.reduce((n, b) => n + 4 + b.byteLength, UI_BATCH_HEADER_BYTES);
  const out = new Uint8Array(size);
  const view = new DataView(out.buffer);
  out.set(UI_BATCH_MAGIC, 0);
  view.setUint16(4, UI_BATCH_VERSION, true);
  view.setUint16(6, flags, true);
  view.setUint32(8, batchSeq, true);
  view.setUint16(12, blocks.length, true);
  view.setUint16(14, 0, true);
  let pos = UI_BATCH_HEADER_BYTES;
  for (const block of blocks) {
    view.setUint32(pos, block.byteLength, true);
    out.set(block, pos + 4);
    pos += 4 + block.byteLength;
  }
  return out;
}

/** Record constructors for fixtures. */
export const rec = {
  f32: (channel: number, value: number): FixtureRecord => ({
    channel,
    quality: Quality.Valid,
    value: { type: ValueType.F32, value },
  }),
  f64: (channel: number, value: number): FixtureRecord => ({
    channel,
    quality: Quality.Valid,
    value: { type: ValueType.F64, value },
  }),
  i32: (channel: number, value: number): FixtureRecord => ({
    channel,
    quality: Quality.Valid,
    value: { type: ValueType.I32, value },
  }),
  u32: (channel: number, value: number): FixtureRecord => ({
    channel,
    quality: Quality.Valid,
    value: { type: ValueType.U32, value },
  }),
  u8: (channel: number, value: number): FixtureRecord => ({
    channel,
    quality: Quality.Valid,
    value: { type: ValueType.U8, value },
  }),
  i8: (channel: number, value: number): FixtureRecord => ({
    channel,
    quality: Quality.Valid,
    value: { type: ValueType.I8, value },
  }),
  bool: (channel: number, value: boolean): FixtureRecord => ({
    channel,
    quality: Quality.Valid,
    value: { type: ValueType.Bool, value },
  }),
  str: (channel: number, value: string): FixtureRecord => ({
    channel,
    quality: Quality.Valid,
    value: { type: ValueType.Str, value },
  }),
  u64: (channel: number, value: bigint): FixtureRecord => ({
    channel,
    quality: Quality.Valid,
    value: { type: ValueType.U64, value },
  }),
  /** A record without a value (INVALID, STALE, UNAVAILABLE, NOT_SUPPORTED). */
  quality: (channel: number, quality: Quality): FixtureRecord => ({ channel, quality, value: NONE }),
};

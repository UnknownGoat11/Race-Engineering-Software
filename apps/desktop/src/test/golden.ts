// Test support: loads the golden vectors written by tools/gen-ts (src/generated/golden) and converts their
// JSON encoding (hex float bit patterns, decimal-string 64-bit integers) back into values.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { ValueType } from '../generated/channels';
import type { CodecError, TlvValue } from '../telemetry/tlv';
import type { UiBatchError } from '../telemetry/uiBatch';

/** Vitest runs with the app directory (apps/desktop) as its working directory. */
const GOLDEN_DIR = join(process.cwd(), 'src', 'generated', 'golden');

/** Header as written by gen-ts: 64-bit integers as decimal strings, the f64 as hex bits. */
export interface GoldenHeader {
  readonly schemaVersion: number;
  readonly captureMonoNs: string;
  readonly captureRelayNsEst: string;
  readonly sourceEtS: string;
}

export type GoldenValue =
  | { readonly type: 'None' }
  | { readonly type: 'F32' | 'F64'; readonly bits: string }
  | { readonly type: 'I32' | 'U32' | 'U8' | 'I8'; readonly value: number }
  | { readonly type: 'Bool'; readonly value: boolean }
  | { readonly type: 'Str'; readonly value: string; readonly utf8: string }
  | { readonly type: 'U64'; readonly value: string };

export interface GoldenRecord {
  readonly channel: number;
  readonly quality: number;
  readonly value: GoldenValue;
}

export type GoldenTlvExpect =
  | { readonly ok: { readonly header: GoldenHeader; readonly records: readonly GoldenRecord[] } }
  | { readonly error: CodecError };

export interface GoldenTlvVector {
  readonly name: string;
  readonly file: string;
  readonly description: string;
  readonly expect: GoldenTlvExpect;
}

export interface GoldenFrame {
  readonly applied: number;
  readonly unknownSkipped: number;
  readonly invalidated: number;
  readonly samples: readonly GoldenRecord[];
}

export type GoldenRejectReason =
  | { readonly kind: 'Codec'; readonly error: CodecError }
  | { readonly kind: 'SchemaMismatch'; readonly found: number; readonly expected: number };

export type GoldenApply =
  | { readonly accepted: GoldenFrame }
  | { readonly rejected: { readonly blockIndex: number; readonly reason: GoldenRejectReason } };

export interface GoldenUiBatchOk {
  readonly version: number;
  readonly flags: number;
  readonly reset: boolean;
  readonly backfill: boolean;
  readonly gapBefore: boolean;
  readonly batchSeq: number;
  readonly blocks: readonly { readonly offset: number; readonly length: number; readonly decode: GoldenTlvExpect }[];
  readonly apply: GoldenApply;
}

export interface GoldenUiBatchVector {
  readonly name: string;
  readonly file: string;
  readonly description: string;
  readonly expect: { readonly ok: GoldenUiBatchOk } | { readonly error: UiBatchError };
}

/** A batch too large to commit: `length` bytes, every one equal to `fill` (see `syntheticBatch`). */
export interface GoldenSyntheticBatchVector {
  readonly name: string;
  readonly description: string;
  readonly length: number;
  readonly fill: number;
  readonly expect: GoldenUiBatchVector['expect'];
}

export interface GoldenSequence {
  readonly name: string;
  readonly description: string;
  readonly files: readonly string[];
  readonly gapBefore: readonly boolean[];
}

export interface GoldenManifest {
  readonly generated: string;
  readonly telemetrySchemaVersion: number;
  readonly uiBatchFormat: {
    readonly magic: string;
    readonly version: number;
    readonly headerBytes: number;
    readonly flagReset: number;
    readonly flagBackfill: number;
    readonly flagGapBefore: number;
    readonly maxBatchBytes: number;
  };
  readonly notes: readonly string[];
  readonly tlv: readonly GoldenTlvVector[];
  readonly uiBatch: readonly GoldenUiBatchVector[];
  readonly uiBatchSynthetic: readonly GoldenSyntheticBatchVector[];
  readonly uiBatchSequences: readonly GoldenSequence[];
}

/** The bytes a synthetic vector describes. */
export function syntheticBatch(vector: GoldenSyntheticBatchVector): Uint8Array {
  return new Uint8Array(vector.length).fill(vector.fill);
}

/** The parsed manifest. The JSON is produced by gen-ts from the Rust codec; its shape is the interface above. */
export function loadManifest(): GoldenManifest {
  return JSON.parse(readFileSync(join(GOLDEN_DIR, 'manifest.json'), 'utf8')) as GoldenManifest;
}

/** Bytes of a golden file (path relative to the golden directory). */
export function loadGoldenFile(file: string): Uint8Array {
  return new Uint8Array(readFileSync(join(GOLDEN_DIR, ...file.split('/'))));
}

/** The vector called `name`; throws if the manifest has no such vector. */
export function findVector<T extends { readonly name: string }>(vectors: readonly T[], name: string): T {
  const vector = vectors.find((v) => v.name === name);
  if (vector === undefined) throw new Error(`golden vector ${name} not found`);
  return vector;
}

const F32_BYTES = 4;
const F64_BYTES = 8;

/** f32 from its hex bit pattern (`0x3f800000`), widened to a double. */
export function f32FromBits(hex: string): number {
  const view = new DataView(new ArrayBuffer(F32_BYTES));
  view.setUint32(0, Number.parseInt(hex, 16));
  return view.getFloat32(0);
}

/** f64 from its hex bit pattern (`0x3ff0000000000000`). */
export function f64FromBits(hex: string): number {
  const view = new DataView(new ArrayBuffer(F64_BYTES));
  view.setBigUint64(0, BigInt(hex));
  return view.getFloat64(0);
}

/** The value a golden record describes, as `decodeTlv` returns it. */
export function goldenToTlvValue(v: GoldenValue): TlvValue {
  switch (v.type) {
    case 'None':
      return { type: ValueType.None };
    case 'F32':
      return { type: ValueType.F32, value: f32FromBits(v.bits) };
    case 'F64':
      return { type: ValueType.F64, value: f64FromBits(v.bits) };
    case 'I32':
      return { type: ValueType.I32, value: v.value };
    case 'U32':
      return { type: ValueType.U32, value: v.value };
    case 'Bool':
      return { type: ValueType.Bool, value: v.value };
    case 'U8':
      return { type: ValueType.U8, value: v.value };
    case 'I8':
      return { type: ValueType.I8, value: v.value };
    case 'Str':
      return { type: ValueType.Str, value: v.value };
    case 'U64':
      return { type: ValueType.U64, value: BigInt(v.value) };
  }
}

/** Bytes of a hex string (`'4c52'` → `[0x4c, 0x52]`). */
export function bytesFromHex(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = Number.parseInt(hex.slice(2 * i, 2 * i + 2), 16);
  }
  return out;
}

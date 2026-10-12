import { describe, expect, it } from 'vitest';
import { MAX_STR_VALUE_BYTES, Quality, RELAY_NS_UNKNOWN, TLV_HEADER_BYTES, ValueType } from '../generated/channels';
import {
  f64FromBits,
  findVector,
  goldenToTlvValue,
  loadGoldenFile,
  loadManifest,
  type GoldenTlvExpect,
} from '../test/golden';
import { encodeTlvBlock, rec } from '../test/tlvEncoder';
import { decodeTlv, readTlvBlock, type DecodedTlvBlock, type TlvRecordVisitor } from './tlv';

const manifest = loadManifest();

/** Compares a decode result with the expectation gen-ts computed from `telemetry_model::codec::decode`. */
function expectMatches(result: ReturnType<typeof decodeTlv>, expected: GoldenTlvExpect): void {
  if ('error' in expected) {
    expect(result).toEqual({ ok: false, error: expected.error });
    return;
  }
  if (!result.ok) throw new Error(`expected a successful decode, got ${JSON.stringify(result.error)}`);
  const block: DecodedTlvBlock = result.value;
  const h = expected.ok.header;
  expect(block.header.schemaVersion).toBe(h.schemaVersion);
  expect(block.header.captureMonoNs).toBe(BigInt(h.captureMonoNs));
  expect(block.header.captureRelayNsEst).toBe(BigInt(h.captureRelayNsEst));
  expect(Object.is(block.header.sourceEtS, f64FromBits(h.sourceEtS))).toBe(true);
  expect(block.records).toHaveLength(expected.ok.records.length);
  expected.ok.records.forEach((want, i) => {
    const got = block.records[i];
    expect(got?.channel).toBe(want.channel);
    expect(got?.quality).toBe(want.quality);
    const wantValue = goldenToTlvValue(want.value);
    expect(got?.value.type).toBe(wantValue.type);
    if (got !== undefined && 'value' in got.value && 'value' in wantValue) {
      // Object.is: -0 and +0 differ, NaN equals NaN (payload bits are not observable through a JS number).
      expect(Object.is(got.value.value, wantValue.value)).toBe(true);
    }
  });
}

describe('golden TLV vectors (produced by the Rust codec)', () => {
  it('covers every error kind the decoder can return', () => {
    const kinds = new Set(manifest.tlv.flatMap((v) => ('error' in v.expect ? [v.expect.error.kind] : [])));
    expect([...kinds].sort()).toEqual(
      ['InvalidUtf8', 'StringTooLong', 'TrailingBytes', 'Truncated', 'UnknownQuality', 'UnknownValueType'].sort(),
    );
    expect(manifest.telemetrySchemaVersion).toBe(1);
  });

  it.each(manifest.tlv.map((v) => [v.name, v] as const))('%s', (_name, vector) => {
    expectMatches(decodeTlv(loadGoldenFile(vector.file)), vector.expect);
  });

  it.each(manifest.tlv.map((v) => [v.name, v] as const))('%s decodes identically at a non-zero offset', (_name, vector) => {
    const bytes = loadGoldenFile(vector.file);
    const padded = new Uint8Array(bytes.byteLength + 13);
    padded.set(bytes, 7);
    expectMatches(decodeTlv(padded.subarray(7, 7 + bytes.byteLength)), vector.expect);
  });
});

describe('truncation and trailing bytes (codec.rs truncation_is_detected_at_every_length)', () => {
  const decodable = manifest.tlv.filter((v) => 'ok' in v.expect);

  it.each(decodable.map((v) => [v.name, v] as const))('%s: every strict prefix fails', (_name, vector) => {
    const bytes = loadGoldenFile(vector.file);
    for (let len = 0; len < bytes.byteLength; len++) {
      expect(decodeTlv(bytes.subarray(0, len)).ok, `prefix of length ${len}`).toBe(false);
    }
    const extra = new Uint8Array(bytes.byteLength + 1);
    extra.set(bytes);
    expect(decodeTlv(extra)).toEqual({ ok: false, error: { kind: 'TrailingBytes', count: 1 } });
  });
});

describe('readTlvBlock visitor', () => {
  it('visits records in order with numbers, booleans, strings and bigints', () => {
    const bytes = loadGoldenFile(findVector(manifest.tlv, 'all_value_types').file);
    const seen: unknown[] = [];
    const visitor: TlvRecordVisitor = {
      record(channel, quality, valueType, value) {
        seen.push([channel, quality, valueType, value]);
      },
    };
    const result = readTlvBlock(new DataView(bytes.buffer, bytes.byteOffset), 0, bytes.byteLength, visitor);
    expect(result.ok && result.value.recordCount).toBe(12);
    expect(seen[0]).toEqual([expect.any(Number), Quality.Valid, ValueType.F32, 71.5]);
    expect(seen[9]).toEqual([expect.any(Number), Quality.Valid, ValueType.U64, 18446744073709551615n]);
    expect(seen[11]).toEqual([expect.any(Number), Quality.NotSupported, ValueType.None, undefined]);
  });

  it('visits the records before a failing one (callers scan with a null visitor first)', () => {
    const bytes = loadGoldenFile(findVector(manifest.tlv, 'err_unknown_quality_after_valid_record').file);
    let visited = 0;
    const result = readTlvBlock(new DataView(bytes.buffer, bytes.byteOffset), 0, bytes.byteLength, {
      record() {
        visited += 1;
      },
    });
    expect(result).toEqual({ ok: false, error: { kind: 'UnknownQuality', quality: 6 } });
    expect(visited).toBe(1);
  });
});

describe('decoder details', () => {
  it('keeps U64 values above 2^53 exact', () => {
    const decoded = decodeTlv(loadGoldenFile(findVector(manifest.tlv, 'integer_extremes').file));
    expect(decoded.ok).toBe(true);
    if (!decoded.ok) return;
    expect(decoded.value.records[8]?.value).toEqual({ type: ValueType.U64, value: 2n ** 53n + 1n });
  });

  it('reports the relay-clock sentinel and a NaN ET as decoded', () => {
    const decoded = decodeTlv(loadGoldenFile(findVector(manifest.tlv, 'empty_block_unknown_clocks').file));
    expect(decoded.ok && decoded.value.header.captureRelayNsEst).toBe(RELAY_NS_UNKNOWN);
    expect(decoded.ok && Number.isNaN(decoded.value.header.sourceEtS)).toBe(true);
  });

  it('keeps a leading U+FEFF and accepts the 64-byte maximum', () => {
    const decoded = decodeTlv(loadGoldenFile(findVector(manifest.tlv, 'strings').file));
    if (!decoded.ok) throw new Error('strings vector must decode');
    const strings = decoded.value.records.map((r) => (r.value.type === ValueType.Str ? r.value.value : null));
    expect(strings).toContain('\u{feff}BOM');
    expect(strings).toContain('x'.repeat(MAX_STR_VALUE_BYTES));
    expect(strings).toContain('\u{1F3C1}'.repeat(MAX_STR_VALUE_BYTES / 4));
  });

  it('never allocates from an untrusted record count', () => {
    // 65 535 records announced in a 28-byte block: fails as truncated at the first record.
    const decoded = decodeTlv(loadGoldenFile(findVector(manifest.tlv, 'err_count_exceeds_bytes').file));
    expect(decoded).toEqual({ ok: false, error: { kind: 'Truncated', offset: TLV_HEADER_BYTES, needed: 2 } });
  });
});

describe('fixture encoder (src/test) matches the Rust encoder byte for byte', () => {
  const roundTrip = [
    'empty_block',
    'empty_block_unknown_clocks',
    'header_extremes',
    'header_extremes_negative',
    'schema_version_2',
    'all_value_types',
    'integer_extremes',
    'every_quality',
    'strings',
    'unknown_channel_ids',
  ];

  it.each(roundTrip)('%s', (name) => {
    const bytes = loadGoldenFile(findVector(manifest.tlv, name).file);
    const decoded = decodeTlv(bytes);
    if (!decoded.ok) throw new Error(`${name} must decode`);
    const { header, records } = decoded.value;
    const again = encodeTlvBlock(header, records);
    expect(again).toEqual(bytes);
  });

  it('drops the value of non-VALID samples and writes VALID-without-value as UNAVAILABLE', () => {
    const block = encodeTlvBlock({ sourceEtS: 1 }, [
      { channel: 1, quality: Quality.Invalid, value: { type: ValueType.F32, value: 999 } },
      { channel: 2, quality: Quality.Valid, value: { type: ValueType.None } },
      rec.f32(3, 1.5),
    ]);
    const decoded = decodeTlv(block);
    expect(decoded.ok && decoded.value.records).toEqual([
      { channel: 1, quality: Quality.Invalid, value: { type: ValueType.None } },
      { channel: 2, quality: Quality.Unavailable, value: { type: ValueType.None } },
      { channel: 3, quality: Quality.Valid, value: { type: ValueType.F32, value: 1.5 } },
    ]);
  });
});

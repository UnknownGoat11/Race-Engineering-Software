import { describe, expect, it } from 'vitest';
import { bytesFromHex, f64FromBits, findVector, loadGoldenFile, loadManifest, syntheticBatch } from '../test/golden';
import { encodeTlvBlock, encodeUiBatch, rec } from '../test/tlvEncoder';
import { decodeTlv } from './tlv';
import {
  UI_BATCH_FLAG_BACKFILL,
  UI_BATCH_FLAG_GAP_BEFORE,
  UI_BATCH_FLAG_RESET,
  UI_BATCH_HEADER_BYTES,
  UI_BATCH_MAGIC,
  UI_BATCH_MAX_BYTES,
  UI_BATCH_VERSION,
  parseUiBatch,
  type UiBatchError,
} from './uiBatch';

const manifest = loadManifest();

describe('UiBatch constants match the Rust reference (D-023)', () => {
  it('uses the same magic, version, header size, flags and size limit', () => {
    const f = manifest.uiBatchFormat;
    expect(UI_BATCH_MAGIC).toEqual([...new TextEncoder().encode(f.magic)]);
    expect(UI_BATCH_VERSION).toBe(f.version);
    expect(UI_BATCH_HEADER_BYTES).toBe(f.headerBytes);
    expect(UI_BATCH_FLAG_RESET).toBe(f.flagReset);
    expect(UI_BATCH_FLAG_BACKFILL).toBe(f.flagBackfill);
    expect(UI_BATCH_FLAG_GAP_BEFORE).toBe(f.flagGapBefore);
    expect(UI_BATCH_MAX_BYTES).toBe(f.maxBatchBytes);
  });
});

describe('golden UiBatch vectors', () => {
  it('covers every structural error (file vectors plus synthetic ones too large to commit)', () => {
    const kinds = new Set(
      [...manifest.uiBatch, ...manifest.uiBatchSynthetic].flatMap((v) =>
        'error' in v.expect ? [v.expect.error.kind] : [],
      ),
    );
    const all: readonly UiBatchError['kind'][] = [
      'BadMagic',
      'ReservedNonZero',
      'TooLarge',
      'TrailingBytes',
      'Truncated',
      'UnknownFlags',
      'UnsupportedVersion',
    ];
    expect([...kinds].sort()).toEqual([...all].sort());
  });

  it.each(manifest.uiBatchSynthetic.map((v) => [v.name, v] as const))('synthetic %s', (_name, vector) => {
    const parsed = parseUiBatch(syntheticBatch(vector));
    if (!('error' in vector.expect)) throw new Error('synthetic vectors describe refused batches');
    expect(parsed).toEqual({ ok: false, error: vector.expect.error });
  });

  it.each(manifest.uiBatch.map((v) => [v.name, v] as const))('%s', (_name, vector) => {
    const bytes = loadGoldenFile(vector.file);
    const parsed = parseUiBatch(bytes);
    if ('error' in vector.expect) {
      expect(parsed).toEqual({ ok: false, error: vector.expect.error });
      return;
    }
    const want = vector.expect.ok;
    if (!parsed.ok) throw new Error(`expected a valid batch, got ${JSON.stringify(parsed.error)}`);
    const batch = parsed.value;
    expect({
      version: batch.version,
      flags: batch.flags,
      reset: batch.reset,
      backfill: batch.backfill,
      gapBefore: batch.gapBefore,
      batchSeq: batch.batchSeq,
    }).toEqual({
      version: want.version,
      flags: want.flags,
      reset: want.reset,
      backfill: want.backfill,
      gapBefore: want.gapBefore,
      batchSeq: want.batchSeq,
    });
    expect(batch.blocks).toEqual(want.blocks.map((b) => ({ offset: b.offset, length: b.length })));
    // Every block decodes (or fails) exactly as the Rust codec says.
    batch.blocks.forEach((block, i) => {
      const expected = want.blocks[i]?.decode;
      const decoded = decodeTlv(bytes.subarray(block.offset, block.offset + block.length));
      if (expected === undefined) throw new Error('block count mismatch');
      if ('error' in expected) {
        expect(decoded).toEqual({ ok: false, error: expected.error });
      } else {
        expect(decoded.ok).toBe(true);
        if (decoded.ok) {
          expect(decoded.value.records).toHaveLength(expected.ok.records.length);
          expect(Object.is(decoded.value.header.sourceEtS, f64FromBits(expected.ok.header.sourceEtS))).toBe(true);
        }
      }
    });
  });

  it('reports bad magic bytes in hex', () => {
    const parsed = parseUiBatch(loadGoldenFile(findVector(manifest.uiBatch, 'err_bad_magic').file));
    expect(parsed.ok).toBe(false);
    if (!parsed.ok && parsed.error.kind === 'BadMagic') {
      expect(new TextDecoder().decode(bytesFromHex(parsed.error.found))).toBe('LRUX');
    }
  });
});

describe('envelope edge cases', () => {
  const block = encodeTlvBlock({ sourceEtS: 1 }, [rec.f32(1, 2)]);
  const batch = encodeUiBatch(UI_BATCH_FLAG_RESET, 7, [block, block]);

  it('rejects every strict prefix of a valid batch and any trailing byte', () => {
    expect(parseUiBatch(batch).ok).toBe(true);
    for (let len = 0; len < batch.byteLength; len++) {
      expect(parseUiBatch(batch.subarray(0, len)).ok, `prefix ${len}`).toBe(false);
    }
    const extra = new Uint8Array(batch.byteLength + 1);
    extra.set(batch);
    expect(parseUiBatch(extra)).toEqual({ ok: false, error: { kind: 'TrailingBytes', count: 1 } });
  });

  it('parses a batch that is a view into a larger buffer', () => {
    const padded = new Uint8Array(batch.byteLength + 9);
    padded.set(batch, 5);
    const parsed = parseUiBatch(padded.subarray(5, 5 + batch.byteLength));
    expect(parsed.ok && parsed.value.blocks).toEqual([
      { offset: 20, length: block.byteLength },
      { offset: 24 + block.byteLength, length: block.byteLength },
    ]);
  });

  it('refuses a batch above 4 MiB before reading it, and accepts the limit itself as a size', () => {
    const tooLarge = new Uint8Array(UI_BATCH_MAX_BYTES + 1);
    expect(parseUiBatch(tooLarge)).toEqual({
      ok: false,
      error: { kind: 'TooLarge', length: UI_BATCH_MAX_BYTES + 1, max: UI_BATCH_MAX_BYTES },
    });
    // Exactly at the limit the size is fine; zero bytes then fail the magic check.
    expect(parseUiBatch(new Uint8Array(UI_BATCH_MAX_BYTES))).toEqual({
      ok: false,
      error: { kind: 'BadMagic', found: '00000000' },
    });
  });

  it('never trusts block_count for allocation', () => {
    const header = encodeUiBatch(0, 1, []);
    new DataView(header.buffer).setUint16(12, 0xffff, true);
    expect(parseUiBatch(header)).toEqual({ ok: false, error: { kind: 'Truncated', offset: 16, needed: 4 } });
  });

  it('accepts a zero-length block (its TLV decode fails later, rejecting the batch)', () => {
    const parsed = parseUiBatch(encodeUiBatch(0, 1, [new Uint8Array(0)]));
    expect(parsed.ok && parsed.value.blocks).toEqual([{ offset: 20, length: 0 }]);
    expect(decodeTlv(new Uint8Array(0))).toEqual({ ok: false, error: { kind: 'Truncated', offset: 0, needed: 2 } });
  });
});

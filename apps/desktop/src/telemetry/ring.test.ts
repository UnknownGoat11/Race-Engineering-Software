// Ports the scenarios of telemetry-model's ring.rs tests, plus the TypeScript-only additions.
import { describe, expect, it } from 'vitest';
import { Quality, ch } from '../generated/channels';
import { MAX_RING_BYTES, TelemetryRingBuffer } from './ring';

function ring(capacity: number, channels: readonly number[] = [ch.SPEED, ch.GEAR]): TelemetryRingBuffer {
  const created = TelemetryRingBuffer.create(capacity, channels);
  if (!created.ok) throw new Error(`ring: ${created.error.kind}`);
  return created.value;
}

function push(r: TelemetryRingBuffer, t: number, values: readonly (number | null)[]): void {
  const error = r.pushRow(t, values);
  if (error !== null) throw new Error(`push: ${error.kind}`);
}

describe('construction errors', () => {
  it('mirrors RingError for capacity and channel problems', () => {
    const fail = (capacity: number, channels: readonly number[]) => {
      const created = TelemetryRingBuffer.create(capacity, channels);
      return created.ok ? null : created.error;
    };
    expect(fail(0, [ch.SPEED])).toEqual({ kind: 'ZeroCapacity' });
    expect(fail(4, [ch.SPEED, ch.SPEED])).toEqual({ kind: 'DuplicateChannel', channelId: ch.SPEED });
    expect(fail(4, [0xffff])).toEqual({ kind: 'UnknownChannel', channelId: 0xffff });
    expect(fail(4, [ch.VEHICLE_NAME])).toEqual({ kind: 'NonNumericChannel', channelId: ch.VEHICLE_NAME });
    for (const bad of [-1, 2.5, Number.NaN, Number.POSITIVE_INFINITY]) {
      expect(fail(bad, [ch.SPEED])).toEqual({ kind: 'InvalidCapacity', capacity: bad });
    }
  });
});

describe('append, wrap and keep newest', () => {
  it('wraps_and_keeps_newest', () => {
    const r = ring(3);
    for (let i = 0; i < 5; i++) push(r, i, [i * 10, i]);
    expect(r.length).toBe(3);
    expect([...r.times()]).toEqual([2, 3, 4]);
    expect(r.oldest(ch.SPEED)).toEqual([2, 20]);
    expect(r.newest(ch.SPEED)).toEqual([4, 40]);
    expect(r.pushRow(3.5, [null, null])).toEqual({ kind: 'TimeRegression', t: 3.5, newest: 4 });
    expect(r.pushRow(Number.NaN, [null, null])).toEqual({ kind: 'NonFiniteTime', t: Number.NaN });
    expect(r.pushRow(5, [null])).toEqual({ kind: 'RowWidth', expected: 2, actual: 1 });
    r.clear();
    expect(r.isEmpty).toBe(true);
    expect(r.newest(ch.SPEED)).toBeNull();
  });

  it('keeps order across many wrap-arounds (O(1) head arithmetic, no shifting)', () => {
    const r = ring(7);
    for (let i = 0; i < 1000; i++) push(r, i, [i, i]);
    expect([...r.times()]).toEqual([993, 994, 995, 996, 997, 998, 999]);
    expect(r.valueAt(ch.SPEED, 0)).toBe(993);
    expect(r.valueAt(ch.SPEED, 6)).toBe(999);
    expect(r.valueAt(ch.SPEED, 7)).toBeNull();
  });

  it('accepts equal timestamps (non-decreasing)', () => {
    const r = ring(4);
    push(r, 1, [1, 1]);
    push(r, 1, [2, 2]);
    expect([...r.times()]).toEqual([1, 1]);
  });

  it('stores non-finite values as gaps, never as numbers', () => {
    const r = ring(4);
    push(r, 1, [Number.POSITIVE_INFINITY, Number.NaN]);
    expect(r.valueAt(ch.SPEED, 0)).toBeNull();
    expect(r.valueAt(ch.GEAR, 0)).toBeNull();
  });
});

describe('queries', () => {
  it('range_and_nearest', () => {
    const r = ring(10);
    for (const [t, v] of [
      [0, 1],
      [1, 2],
      [2, 3],
      [3, 4],
    ] as const) {
      push(r, t, [v, v]);
    }
    expect(r.rangeIndices(0.5, 2)).toEqual({ start: 1, end: 3 });
    expect(r.range(ch.SPEED, 1, 2)).toEqual([
      [1, 2],
      [2, 3],
    ]);
    expect(r.range(ch.RPM, 0, 3)).toEqual([]);
    expect(r.nearestIndex(1.4)).toBe(1);
    expect(r.nearestIndex(1.6)).toBe(2);
    expect(r.nearestIndex(1.5)).toBe(1); // tie → earlier row
    expect(r.nearestIndex(-5)).toBe(0);
    expect(r.nearest(ch.SPEED, 99)).toEqual([3, 4]);
    expect(r.rangeIndices(2, 1)).toEqual({ start: 0, end: 0 });
    expect(r.rangeIndices(Number.NaN, 1)).toEqual({ start: 0, end: 0 });
    expect(r.nearestIndex(Number.NaN)).toBeNull();
  });

  it('interpolation_respects_channel_kind_and_gaps', () => {
    const r = ring(10);
    push(r, 0, [10, 2]);
    push(r, 1, [20, 3]);
    push(r, 2, [null, null]);
    const maxGap = 1.5;
    expect(r.sampleAtTime(ch.SPEED, 0.25, maxGap)).toBeCloseTo(12.5, 12);
    expect(r.sampleAtTime(ch.GEAR, 0.75, maxGap)).toBe(2); // discrete: previous sample, never 2.75
    expect(r.sampleAtTime(ch.SPEED, 1.5, maxGap)).toBeNull(); // next to a missing value
    expect(r.sampleAtTime(ch.SPEED, 5, maxGap)).toBeNull(); // outside the span
    expect(r.sampleAtTime(ch.SPEED, 1, maxGap)).toBe(20);
  });

  it('point_queries_never_bridge_time_gaps', () => {
    // A link drop between ET 100.0 (85 m/s, gear 6) and 112.0 (20 m/s, gear 2) must not produce 52.5 m/s at
    // 106.0 or gear 6 at 111.9: neither was ever measured.
    const r = ring(16);
    for (const [t, speed, gear] of [
      [99.98, 85, 6],
      [99.99, 85, 6],
      [100.0, 85, 6],
      [112.0, 20, 2],
      [112.01, 20.5, 2],
    ] as const) {
      push(r, t, [speed, gear]);
    }
    const maxGap = 0.05;
    expect(r.sampleAtTime(ch.SPEED, 106, maxGap)).toBeNull();
    expect(r.sampleAtTime(ch.GEAR, 111.9, maxGap)).toBeNull();
    expect(r.sampleAtTime(ch.GEAR, 100.02, maxGap)).toBe(6); // held within the gap budget
    expect(r.sampleAtTime(ch.SPEED, 112.005, maxGap)).toBeCloseTo(20.25, 9);
    expect(r.sampleAtTime(ch.SPEED, 112, maxGap)).toBe(20);
    for (const bad of [Number.NaN, -1]) {
      expect(r.sampleAtTime(ch.SPEED, 112.005, bad)).toBeNull();
    }
  });

  it('wrapping_channels_are_held_not_interpolated', () => {
    const r = ring(4, [ch.TIME_OF_DAY, ch.LAP_TIME]);
    push(r, 10, [86_399.9, 95.3]);
    push(r, 10.2, [0.1, 0.01]);
    // TIME_OF_DAY is an F32 channel: its column holds the f32 nearest to 86 399.9.
    expect(r.sampleAtTime(ch.TIME_OF_DAY, 10.1, 1)).toBe(Math.fround(86_399.9));
    expect(r.sampleAtTime(ch.LAP_TIME, 10.1, 1)).toBe(95.3);
  });

  it('segments_split_on_missing_values_and_time_gaps', () => {
    const r = ring(16);
    for (const [t, v] of [
      [0, 1],
      [0.1, 1],
      [0.2, null],
      [0.3, 1],
      [5, 1],
      [5.1, 1],
    ] as const) {
      push(r, t, [v, 1]);
    }
    expect(r.segments(ch.SPEED, 0, 10, 0.5)).toEqual([
      { start: 0, end: 2 },
      { start: 3, end: 4 },
      { start: 4, end: 6 },
    ]);
    expect(r.segments(ch.RPM, 0, 10, 0.5)).toEqual([]);
  });

  it('copyRange fills caller-owned arrays with gaps as NaN', () => {
    const r = ring(8);
    push(r, 0, [1, 1]);
    push(r, 1, [null, 1]);
    push(r, 2, [3, 1]);
    const times = new Float64Array(2);
    const values = new Float64Array(2);
    expect(r.copyRange(ch.SPEED, 0, 2, times, values)).toBe(2);
    expect([...times]).toEqual([0, 1]);
    expect(values[0]).toBe(1);
    expect(Number.isNaN(values[1])).toBe(true);
    expect(r.copyRange(ch.RPM, 0, 2, times, values)).toBe(0);
  });
});

describe('capacity limits and resizing', () => {
  it('oversized_capacity_is_an_error_not_an_abort', () => {
    const huge = TelemetryRingBuffer.create(Number.MAX_SAFE_INTEGER, [ch.SPEED]);
    expect(huge.ok ? null : huge.error.kind).toBe('CapacityTooLarge');
    // One row over the byte budget (timestamp + one channel = 2 columns of 8 bytes).
    const rows = MAX_RING_BYTES / (2 * 8) + 1;
    const over = TelemetryRingBuffer.create(rows, [ch.SPEED]);
    expect(over.ok ? null : over.error).toEqual({
      kind: 'CapacityTooLarge',
      capacity: rows,
      columns: 2,
      maxBytes: MAX_RING_BYTES,
    });
    const r = ring(4);
    push(r, 1, [1, 1]);
    expect(r.resize(Number.MAX_SAFE_INTEGER)?.kind).toBe('CapacityTooLarge');
    expect([r.capacity, r.length, r.valueAt(ch.SPEED, 0)]).toEqual([4, 1, 1]); // unchanged
  });

  it('resize_keeps_newest_rows_in_order', () => {
    const r = ring(4);
    for (let i = 0; i < 6; i++) push(r, i, [i, i]);
    expect(r.resize(2)).toBeNull();
    expect([...r.times()]).toEqual([4, 5]);
    expect(r.resize(5)).toBeNull();
    expect(r.capacity).toBe(5);
    push(r, 6, [6, 6]);
    expect([...r.times()]).toEqual([4, 5, 6]);
    expect(r.valueAt(ch.SPEED, 2)).toBe(6);
    expect(r.resize(0)).toEqual({ kind: 'ZeroCapacity' });
  });
});

describe('store fast paths', () => {
  it('pushRowGather stores only valid values (push_frame_stores_only_valid_values)', () => {
    const r = ring(4);
    const current = new Float64Array([50, Number.NaN]);
    const sourceIndex = new Int32Array([0, 1]);
    expect(r.pushRowGather(1, current, sourceIndex)).toBeNull();
    expect(r.valueAt(ch.SPEED, 0)).toBe(50);
    expect(r.valueAt(ch.GEAR, 0)).toBeNull(); // unavailable gear is a gap, not 0
    expect(r.pushRowGather(2, current, new Int32Array([0]))).toEqual({ kind: 'RowWidth', expected: 2, actual: 1 });
    expect(r.pushRowGather(0.5, current, sourceIndex)?.kind).toBe('TimeRegression');
  });

  it('replaceNewestRowGather rewrites the newest row in place', () => {
    const r = ring(4);
    const sourceIndex = new Int32Array([0, 1]);
    expect(r.replaceNewestRowGather(new Float64Array([1, 1]), sourceIndex)).toBe(false);
    r.pushRowGather(1, new Float64Array([1, 1]), sourceIndex);
    expect(r.replaceNewestRowGather(new Float64Array([2, Number.NaN]), sourceIndex)).toBe(true);
    expect(r.length).toBe(1);
    expect([r.timeAt(0), r.valueAt(ch.SPEED, 0), r.valueAt(ch.GEAR, 0)]).toEqual([1, 2, null]);
  });

  it('pushGap writes a row in which every channel is a gap', () => {
    const r = ring(4);
    push(r, 1, [1, 1]);
    expect(r.pushGap(1.5)).toBeNull();
    push(r, 2, [2, 2]);
    expect(r.segments(ch.SPEED, 0, 3, 10)).toEqual([
      { start: 0, end: 1 },
      { start: 2, end: 3 },
    ]);
    expect(r.sampleAtTime(ch.SPEED, 1.25, 10)).toBeNull();
  });

  it('keeps F32 values exact in single-precision columns and F64 values in double columns', () => {
    const r = ring(2, [ch.SPEED, ch.LAP_TIME]);
    const f32 = Math.fround(83.123456789);
    push(r, 0, [f32, 83.123456789]);
    expect(r.valueAt(ch.SPEED, 0)).toBe(f32);
    expect(r.valueAt(ch.LAP_TIME, 0)).toBe(83.123456789);
  });

  it('records per-row quality in the ring and preserves it through copyRange and resize (DEFERRED 17)', () => {
    const r = ring(4, [ch.SPEED, ch.GEAR]);
    // Row 0: SPEED valid 100, GEAR invalid
    expect(r.pushRow(1, [100, null], [Quality.Valid, Quality.Invalid])).toBeNull();
    // Row 1: SPEED unavailable, GEAR stale
    expect(r.pushRow(2, [null, null], [Quality.Unavailable, Quality.Stale])).toBeNull();
    // Row 2: SPEED valid 120, GEAR notSupported
    expect(r.pushRow(3, [120, null], [Quality.Valid, Quality.NotSupported])).toBeNull();

    expect(r.qualityAt(ch.SPEED, 0)).toBe(Quality.Valid);
    expect(r.qualityAt(ch.GEAR, 0)).toBe(Quality.Invalid);
    expect(r.qualityAt(ch.SPEED, 1)).toBe(Quality.Unavailable);
    expect(r.qualityAt(ch.GEAR, 1)).toBe(Quality.Stale);
    expect(r.qualityAt(ch.SPEED, 2)).toBe(Quality.Valid);
    expect(r.qualityAt(ch.GEAR, 2)).toBe(Quality.NotSupported);
    expect(r.qualityAt(ch.SPEED, 3)).toBeNull(); // out of bounds
    expect(r.qualityAt(99999, 0)).toBeNull(); // unknown channel

    // copyRange with qualities
    const times = new Float64Array(3);
    const values = new Float64Array(3);
    const qualities = new Uint8Array(3);
    const count = r.copyRange(ch.GEAR, 0.5, 3.5, times, values, qualities);
    expect(count).toBe(3);
    expect(Array.from(qualities)).toEqual([Quality.Invalid, Quality.Stale, Quality.NotSupported]);

    // pushRowGather with sourceQuality
    const currentValues = new Float64Array([130, Number.NaN]);
    const currentQualities = new Uint8Array([Quality.Valid, Quality.Invalid]);
    const sourceIndex = new Int32Array([0, 1]);
    expect(r.pushRowGather(4, currentValues, sourceIndex, currentQualities)).toBeNull();
    expect(r.qualityAt(ch.SPEED, 3)).toBe(Quality.Valid);
    expect(r.qualityAt(ch.GEAR, 3)).toBe(Quality.Invalid);

    // replaceNewestRowGather with sourceQuality
    currentValues[0] = 135;
    currentQualities[0] = Quality.Stale;
    expect(r.replaceNewestRowGather(currentValues, sourceIndex, currentQualities)).toBe(true);
    expect(r.qualityAt(ch.SPEED, 3)).toBe(Quality.Stale);

    // resize preserves qualities
    expect(r.resize(6)).toBeNull();
    expect(r.qualityAt(ch.SPEED, 3)).toBe(Quality.Stale);
    expect(r.qualityAt(ch.GEAR, 3)).toBe(Quality.Invalid);
  });
});

import { describe, expect, it, vi } from 'vitest';
import { ch } from '../generated/channels';
import { FakeClock, FakeFrames } from '../test/fakeScheduler';
import { encodeTlvBlock, encodeUiBatch, rec, type FixtureRecord } from '../test/tlvEncoder';
import { TelemetryStore } from '../telemetry/store';
import { UI_BATCH_FLAG_RESET } from '../telemetry/uiBatch';
import { LiveRingSource, advanceLapScan, newLapScan, type LapScanState, type LiveRingSourceOptions } from './liveSource';

const ROW_DT = 0.01;

interface Harness {
  readonly store: TelemetryStore;
  readonly clock: FakeClock;
  readonly frames: FakeFrames;
  /** Applies one batch with one block per `[et, records]`; the first batch (or `reset`) carries RESET. */
  send(blocks: readonly (readonly [number, readonly FixtureRecord[]])[], reset?: boolean): void;
}

function harness(liveWindowS?: number): Harness {
  const clock = new FakeClock();
  const frames = new FakeFrames();
  const store = new TelemetryStore({
    now: clock.now,
    scheduleFrame: frames.schedule,
    scheduleTimeout: clock.scheduleTimeout,
    ...(liveWindowS === undefined ? {} : { liveWindowS }),
  });
  let seq = -1;
  return {
    store,
    clock,
    frames,
    send(blocks, reset = false) {
      seq += 1;
      const flags = seq === 0 || reset ? UI_BATCH_FLAG_RESET : 0;
      const encoded = blocks.map(([sourceEtS, records]) => encodeTlvBlock({ sourceEtS }, records));
      const result = store.applyBatch(encodeUiBatch(flags, seq, encoded));
      if (!result.ok) throw new Error(`batch refused: ${JSON.stringify(result.error)}`);
    },
  };
}

/** `n` rows from `t0` every 10 ms, with speed = row index. */
function speedRows(t0: number, n: number): [number, FixtureRecord[]][] {
  return Array.from({ length: n }, (_, i) => [t0 + i * ROW_DT, [rec.f32(ch.SPEED, i)]] as [number, FixtureRecord[]]);
}

describe('LiveRingSource on the time axis', () => {
  it('returns the rows of the range plus one row past each edge, x = session time', () => {
    const h = harness();
    h.send(speedRows(10, 100));
    const source = new LiveRingSource(h.store, { xMode: 'time', dataKind: 'lmu' });
    const w = source.read({ min: 10.2, max: 10.5 }, [ch.SPEED]);
    expect(w.xs).toBe(w.ts);
    expect(w.monotonicX).toBe(true);
    // Rows 20…50 are inside, rows 19 and 51 are the edge rows.
    expect(w.count).toBe(33);
    expect(w.ts[0]).toBeCloseTo(10.19, 9);
    expect(w.ts[32]).toBeCloseTo(10.51, 9);
    const speed = w.column(ch.SPEED);
    expect(speed?.[0]).toBe(19);
    expect(speed?.[32]).toBe(51);
  });

  it('reuses its buffers between reads of the same size', () => {
    const h = harness();
    h.send(speedRows(10, 100));
    const source = new LiveRingSource(h.store, { xMode: 'time', dataKind: 'lmu' });
    const a = source.read({ min: 10, max: 10.5 }, [ch.SPEED]);
    const ts = a.ts;
    const column = a.column(ch.SPEED);
    const b = source.read({ min: 10.3, max: 10.9 }, [ch.SPEED]);
    expect(b.ts).toBe(ts);
    expect(b.column(ch.SPEED)).toBe(column);
  });

  it('grows its buffers when a window needs more rows', () => {
    const h = harness();
    h.send(speedRows(0, 3000));
    const source = new LiveRingSource(h.store, { xMode: 'time', dataKind: 'lmu' });
    const w = source.read({ min: 0, max: 30 }, [ch.SPEED]);
    expect(w.count).toBe(3000);
    expect(w.column(ch.SPEED)?.[2999]).toBe(2999);
  });

  it('gives no column for channels that are not in the ring and still returns the times', () => {
    const h = harness();
    h.send(speedRows(10, 10));
    const source = new LiveRingSource(h.store, { xMode: 'time', dataKind: 'lmu' });
    const w = source.read({ min: 10, max: 11 }, [ch.TRACK_LENGTH]);
    expect(w.column(ch.TRACK_LENGTH)).toBeNull();
    expect(w.column(ch.SPEED)).toBeNull();
    expect(w.count).toBe(10);
    expect(w.ts[9]).toBeCloseTo(10.09, 9);
  });

  it('keeps missing values as NaN (never 0)', () => {
    const h = harness();
    h.send([
      [1, [rec.f32(ch.SPEED, 10)]],
      [1.01, [rec.quality(ch.SPEED, 3)]],
      [1.02, [rec.f32(ch.SPEED, 12)]],
    ]);
    const source = new LiveRingSource(h.store, { xMode: 'time', dataKind: 'lmu' });
    const speed = source.read({ min: 0, max: 2 }, [ch.SPEED]).column(ch.SPEED);
    expect(Array.from(speed?.subarray(0, 3) ?? [])).toEqual([10, Number.NaN, 12]);
  });

  it('reports the extent and the current position, stale once the feed stops', () => {
    const h = harness();
    const source = new LiveRingSource(h.store, { xMode: 'time', dataKind: 'lmu' });
    expect(source.extent()).toBeNull();
    expect(source.current()).toBeNull();
    h.send(speedRows(10, 50));
    expect(source.extent()?.min).toBe(10);
    expect(source.extent()?.max).toBeCloseTo(10.49, 9);
    expect(source.current()).toEqual({ x: source.extent()?.max, t: source.extent()?.max, stale: false, degraded: false });
    h.clock.advance(5_000);
    expect(source.current()?.stale).toBe(true);
  });

  it('notifies subscribers when the ring changes and stops after unsubscribe', () => {
    const h = harness();
    const source = new LiveRingSource(h.store, { xMode: 'time', dataKind: 'lmu' });
    const listener = vi.fn();
    const off = source.subscribe(listener);
    h.send(speedRows(10, 5));
    h.frames.runFrame();
    expect(listener).toHaveBeenCalled();
    listener.mockClear();
    off();
    h.send(speedRows(11, 5));
    h.frames.runFrame();
    expect(listener).not.toHaveBeenCalled();
  });
});

describe('LiveRingSource on a lap distance axis', () => {
  it('returns only the current lap, with x from the x channel', () => {
    const h = harness();
    const blocks: [number, FixtureRecord[]][] = [];
    let t = 100;
    for (let d = 4900; d <= 5000; d += 10) {
      blocks.push([t, [rec.f64(ch.LAP_DISTANCE, d), rec.f32(ch.SPEED, 60)]]);
      t += ROW_DT;
    }
    for (let d = 0; d <= 300; d += 10) {
      blocks.push([t, [rec.f64(ch.LAP_DISTANCE, d), rec.f32(ch.SPEED, 61)]]);
      t += ROW_DT;
    }
    h.send(blocks);
    const source = new LiveRingSource(h.store, { xMode: 'lapDistance', dataKind: 'lmu', xChannel: ch.LAP_DISTANCE });
    expect(source.extent()).toEqual({ min: 0, max: 300 });
    const w = source.read({ min: 0, max: 5000 }, [ch.SPEED]);
    expect(w.count).toBe(31);
    expect(w.xs).not.toBe(w.ts);
    expect(w.xs[0]).toBe(0);
    expect(w.xs[30]).toBe(300);
    expect(w.monotonicX).toBe(true);
    expect(w.column(ch.SPEED)?.[0]).toBeCloseTo(61, 5);
    expect(source.current()).toMatchObject({ x: 300, stale: false, degraded: false });
  });
});

/** One block per row from `t0` every 10 ms: lap distance `xs[i]` and speed `i`. */
function lapRows(t0: number, xs: readonly number[]): [number, FixtureRecord[]][] {
  return xs.map((x, i) => [t0 + i * ROW_DT, [rec.f64(ch.LAP_DISTANCE, x), rec.f32(ch.SPEED, i)]] as [number, FixtureRecord[]]);
}

function lapSource(h: Harness): LiveRingSource {
  return new LiveRingSource(h.store, { xMode: 'lapDistance', dataKind: 'lmu', xChannel: ch.LAP_DISTANCE });
}

/** 0, step, 2·step, … (n values). */
function ramp(n: number, step: number, from = 0): number[] {
  return Array.from({ length: n }, (_, i) => from + i * step);
}

describe('LiveRingSource lap scan', () => {
  it('reads only the visible rows of a monotonic lap (plus one row past each edge)', () => {
    const h = harness();
    h.send(lapRows(100, ramp(101, 10))); // x = 0 … 1000
    const source = lapSource(h);
    const w = source.read({ min: 200, max: 300 }, [ch.SPEED]);
    expect(w.count).toBe(13);
    expect(w.xs[0]).toBe(190);
    expect(w.xs[12]).toBe(310);
    expect(w.column(ch.SPEED)?.[0]).toBe(19);
  });

  it('follows the lap across batches and finds a lap reset in a later batch', () => {
    const h = harness();
    const source = lapSource(h);
    h.send(lapRows(100, ramp(50, 100))); // 0 … 4900
    expect(source.extent()).toEqual({ min: 0, max: 4900 });
    h.send(lapRows(100.5, ramp(10, 10, 4950))); // 4950 … 5040
    expect(source.extent()).toEqual({ min: 0, max: 5040 });
    h.send(lapRows(100.6, ramp(20, 10))); // new lap: 0 … 190
    expect(source.extent()).toEqual({ min: 0, max: 190 });
    const w = source.read({ min: 0, max: 5000 }, [ch.SPEED]);
    expect(w.count).toBe(20);
    expect(w.xs[0]).toBe(0);
  });

  it('rescans a rewritten newest row instead of counting it twice', () => {
    const h = harness();
    const source = lapSource(h);
    h.send(lapRows(100, [0, 10, 20, 30]));
    expect(source.extent()).toEqual({ min: 0, max: 30 });
    // Same ET as the newest row: the store replaces that row (one row per ET).
    h.send([[100 + 3 * ROW_DT, [rec.f64(ch.LAP_DISTANCE, 25)]]]);
    expect(source.extent()).toEqual({ min: 0, max: 25 });
    h.send(lapRows(100.04, [40]));
    expect(source.extent()).toEqual({ min: 0, max: 40 });
    expect(source.read({ min: 0, max: 100 }, [ch.SPEED]).count).toBe(5);
  });

  it('limits the extent to the rows still in the ring when the lap is longer than the live window', () => {
    const h = harness(30); // 3001 rows at the nominal 100 Hz
    const source = lapSource(h);
    h.send(lapRows(100, ramp(4000, 1))); // 40 s of one lap, x = row index
    const oldestX = 4000 - h.store.ring.length;
    expect(source.extent()).toEqual({ min: oldestX, max: 3999 });
    const w = source.read({ min: 0, max: 4000 }, [ch.SPEED]);
    expect(w.count).toBe(h.store.ring.length);
    expect(w.xs[0]).toBe(oldestX);
  });

  it('returns the whole current lap when the car reversed (x not monotonic)', () => {
    const h = harness();
    const xs = [...ramp(50, 10), ...ramp(10, -10, 480), ...ramp(30, 10, 400)]; // 0…490, back to 390, then 400…690
    h.send(lapRows(100, xs));
    const source = lapSource(h);
    const w = source.read({ min: 600, max: 650 }, [ch.SPEED]);
    expect(w.monotonicX).toBe(false);
    expect(w.count).toBe(xs.length);
    expect(source.extent()).toEqual({ min: 0, max: 690 });
  });

  it('rescans from scratch when the ring was cleared and rebuilt with other rows (RESET)', () => {
    const h = harness();
    const source = lapSource(h);
    h.send(lapRows(100, ramp(100, 10))); // 0 … 990
    expect(source.extent()).toEqual({ min: 0, max: 990 });
    h.send(lapRows(100, ramp(100, 10, 5000)), true); // same times, different lap: 5000 … 5990
    expect(source.extent()).toEqual({ min: 5000, max: 5990 });
  });

  it('keeps gap rows inside the lap (no x) without ending it or breaking the binary search', () => {
    const h = harness();
    h.send(lapRows(100, ramp(20, 10)));
    h.send([[100.5, [rec.quality(ch.LAP_DISTANCE, 3)]]]); // x invalid at this row
    h.send(lapRows(100.51, ramp(20, 10, 200)));
    const source = lapSource(h);
    expect(source.extent()).toEqual({ min: 0, max: 390 });
    const w = source.read({ min: 195, max: 215 }, [ch.SPEED]);
    // 190 (edge), the row without x, 200, 210, 220 (edge).
    expect(Array.from(w.xs.subarray(0, w.count))).toEqual([190, Number.NaN, 200, 210, 220]);
  });
});

describe('advanceLapScan', () => {
  function scan(rows: readonly number[]): LapScanState {
    const state = newLapScan();
    rows.forEach((x, i) => {
      advanceLapScan(state, x, i);
    });
    return state;
  }

  it('starts a new lap at an x drop larger than half the largest |x| seen', () => {
    expect(scan([10, 20, 30])).toMatchObject({ firstT: 0, min: 10, max: 30, monotonic: true });
    expect(scan([4000, 4900, 5, 100, 4950, 3, 50])).toMatchObject({ firstT: 5, min: 3, max: 50 });
    expect(scan([0.98, 0.99, Number.NaN, 0.01, 0.02])).toMatchObject({ firstT: 3, min: 0.01, max: 0.02 });
  });

  it('treats small backward steps as jitter and larger ones as a non-monotonic lap', () => {
    expect(scan([1000, 999.5, 1001])).toMatchObject({ firstT: 0, monotonic: true });
    expect(scan([1000, 900, 1001])).toMatchObject({ firstT: 0, monotonic: false, min: 900 });
  });

  it('ignores rows without x and starts empty', () => {
    expect(scan([])).toMatchObject({ firstT: Number.NaN, min: Number.POSITIVE_INFINITY });
    expect(scan([Number.NaN, Number.NaN])).toMatchObject({ firstT: Number.NaN, lastX: Number.NaN });
    expect(scan([Number.NaN, 5, 6])).toMatchObject({ firstT: 1, min: 5, max: 6 });
  });
});

describe('LiveRingSource data kind (silent-failure H2 & DEFERRED 18)', () => {
  it('requires an explicit data kind and rejects anything else', () => {
    const h = harness();
    expect(new LiveRingSource(h.store, { xMode: 'time', dataKind: 'synthetic' }).dataKind).toBe('synthetic');
    expect(() => new LiveRingSource(h.store, { xMode: 'time' } as unknown as LiveRingSourceOptions)).toThrow(TypeError);
  });

  it('reads dataKind from store origin if options.dataKind is omitted (DEFERRED 18)', () => {
    const h = harness();
    h.store.setOrigin('lmu');
    const source = new LiveRingSource(h.store, { xMode: 'time' });
    expect(source.dataKind).toBe('lmu');
    h.store.setOrigin('synthetic');
    expect(source.dataKind).toBe('synthetic');
  });

  it('provides per-row quality buffer via window.quality (DEFERRED 17)', () => {
    const h = harness();
    h.store.setOrigin('lmu');
    h.send([
      [100, [rec.f32(ch.SPEED, 50)]],
      [101, [rec.quality(ch.SPEED, 1)]], // Invalid
    ]);
    const source = new LiveRingSource(h.store, { xMode: 'time' });
    const window = source.read({ min: 100, max: 101 }, [ch.SPEED]);
    expect(window.count).toBe(2);
    const qualities = window.quality?.(ch.SPEED);
    expect(qualities).toBeDefined();
    expect(qualities?.[0]).toBe(0); // Valid
    expect(qualities?.[1]).toBe(1); // Invalid
  });
});

describe('LiveRingSource x channel (silent-failure M3)', () => {
  it('refuses a lap axis without an x channel or with one whose unit does not fit, instead of NO DATA forever', () => {
    const h = harness();
    const missing = { xMode: 'lapDistance', dataKind: 'lmu' } as unknown as LiveRingSourceOptions;
    expect(() => new LiveRingSource(h.store, missing)).toThrow(/needs an xChannel/);
    expect(() => new LiveRingSource(h.store, { xMode: 'lapDistance', xChannel: ch.LAP_TIME, dataKind: 'lmu' })).toThrow(
      /does not fit the 'lapDistance' axis/,
    );
    expect(() => new LiveRingSource(h.store, { xMode: 'lapTime', xChannel: 0x7fff, dataKind: 'lmu' })).toThrow(/not a known channel/);
    expect(() => new LiveRingSource(h.store, { xMode: 'lapFraction', xChannel: ch.LAP_PROGRESS, dataKind: 'lmu' })).not.toThrow();
    expect(() => new LiveRingSource(h.store, { xMode: 'lapTime', xChannel: ch.LAP_TIME, dataKind: 'lmu' })).not.toThrow();
  });
});

describe('LiveRingSource reference lap buffer (REDESIGN.md §4 W5.1, G01, G02)', () => {
  it('captures completed lap on lap wrap and exposes referenceSource (lap wrap handling)', () => {
    const h = harness();
    const source = lapSource(h);
    expect(source.referenceSource()).toBeNull();

    // Lap 1: 0 … 5000 m (101 points)
    h.send(lapRows(100, ramp(51, 100))); // 0 … 5000
    // Lap wrap: drops from 5000 to 0 (new lap)
    h.send(lapRows(100.6, ramp(20, 100))); // 0 … 1900
    source.extent(); // Trigger lap scan

    const ref = source.referenceSource();
    expect(ref).not.toBeNull();
    expect(ref?.origin).toBe('live');
    expect(ref?.extent()).toEqual({ min: 0, max: 5000 });
  });

  it('aligns reference lap by distance allowing direct distance comparison (reference alignment by distance)', () => {
    const h = harness();
    const source = lapSource(h);

    // Lap 1: 0 … 5000 m, speed = 50 m/s everywhere
    const lap1 = ramp(51, 100).map((x, i) => [
      100 + i * ROW_DT,
      [rec.f64(ch.LAP_DISTANCE, x), rec.f32(ch.SPEED, 50)],
    ] as [number, FixtureRecord[]]);
    h.send(lap1);

    // Lap 2 starts: 0 … 3000 m, speed = 60 m/s everywhere
    const lap2 = ramp(31, 100).map((x, i) => [
      101 + i * ROW_DT,
      [rec.f64(ch.LAP_DISTANCE, x), rec.f32(ch.SPEED, 60)],
    ] as [number, FixtureRecord[]]);
    h.send(lap2);

    source.extent(); // Trigger scan
    const ref = source.referenceSource();
    expect(ref).not.toBeNull();

    // Query distance interval 1000m … 2000m from both live source and reference lap
    const currentWindow = source.read({ min: 1000, max: 2000 }, [ch.SPEED]);
    if (!ref) throw new Error('Expected reference source to be present');
    const refWindow = ref.read({ min: 1000, max: 2000 }, [ch.SPEED]);

    // Both traces have xs representing distance in metres
    expect(currentWindow.xs[0]).toBeLessThanOrEqual(1000);
    expect(currentWindow.xs[currentWindow.count - 1]).toBeGreaterThanOrEqual(2000);
    expect(refWindow.xs[0]).toBeLessThanOrEqual(1000);
    expect(refWindow.xs[refWindow.count - 1]).toBeGreaterThanOrEqual(2000);

    // Speed traces reflect their respective laps at the same distance
    expect(currentWindow.column(ch.SPEED)?.[5]).toBeCloseTo(60, 1);
    expect(refWindow.column(ch.SPEED)?.[5]).toBeCloseTo(50, 1);

    // Reference series helper
    const refSeries = source.referenceSeries(ch.SPEED);
    expect(refSeries).toMatchObject({
      key: `ref-${ch.SPEED}`,
      channelId: ch.SPEED,
      source: ref,
    });
  });

  it('supports dynamic setXMode without losing reference lap buffer', () => {
    const h = harness();
    const source = lapSource(h);

    // Complete a lap
    h.send(lapRows(100, ramp(51, 100)));
    h.send(lapRows(100.6, ramp(10, 100)));
    source.extent();

    expect(source.referenceSource()).not.toBeNull();
    const ref = source.referenceSource();

    // Switch to time axis
    source.setXMode('time');
    expect(source.extent()).not.toBeNull();
    // Reference lap remains intact
    expect(source.referenceSource()).toBe(ref);

    // Switch back to lapDistance
    source.setXMode('lapDistance', ch.LAP_DISTANCE);
    expect(source.referenceSource()).toBe(ref);
  });
});



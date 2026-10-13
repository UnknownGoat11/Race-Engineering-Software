import { describe, expect, it } from 'vitest';
import { ch } from '../generated/channels';
import { TelemetryRingBuffer } from '../telemetry/ring';
import { nearestRowX, sampleAt } from './sample';
import { StaticSource } from './staticSource';
import type { GraphWindow } from './types';

const ALL = { min: Number.NEGATIVE_INFINITY, max: Number.POSITIVE_INFINITY };

function lcg(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

function column(window: GraphWindow, id: number): Float64Array {
  const c = window.column(id);
  if (c === null) throw new Error(`no column ${id}`);
  return c;
}

describe('sampleAt on a time axis', () => {
  it('gives exactly the answers of TelemetryRingBuffer.sampleAtTime (Continuous and Discrete)', () => {
    const rand = lcg(7);
    const created = TelemetryRingBuffer.create(400, [ch.SPEED, ch.GEAR]);
    if (!created.ok) throw new Error('ring');
    const ring = created.value;
    let t = 100;
    for (let i = 0; i < 300; i++) {
      t += rand() < 0.05 ? 0.3 + rand() : 0.01;
      const speed = rand() < 0.1 ? Number.NaN : rand() * 80;
      const gear = rand() < 0.1 ? null : Math.floor(rand() * 7);
      ring.pushRow(t, [speed, gear]);
    }
    const times = ring.times();
    const speedValues = Array.from(times, (_, i) => ring.valueAt(ch.SPEED, i));
    const gearValues = Array.from(times, (_, i) => ring.valueAt(ch.GEAR, i));
    const source = new StaticSource({
      xs: times,
      columns: new Map([
        [ch.SPEED, speedValues],
        [ch.GEAR, gearValues],
      ]),
    });
    const window = source.read(ALL, [ch.SPEED, ch.GEAR]);
    const probes: number[] = [99, t + 1, ...Array.from(times)];
    for (let k = 0; k < 2000; k++) probes.push(100 + rand() * (t - 99));
    for (const maxGap of [0.02, 0.5, 5]) {
      for (const probe of probes) {
        const speed = sampleAt(window, column(window, ch.SPEED), probe, false, maxGap);
        const expectedSpeed = ring.sampleAtTime(ch.SPEED, probe, maxGap);
        if (expectedSpeed === null) expect(speed).toBeNull();
        else expect(speed).toBeCloseTo(expectedSpeed, 9);
        const gear = sampleAt(window, column(window, ch.GEAR), probe, true, maxGap);
        expect(gear).toBe(ring.sampleAtTime(ch.GEAR, probe, maxGap));
      }
    }
  });

  it('returns null for an invalid maxGapT', () => {
    const source = new StaticSource({ xs: [0, 1], columns: new Map([[1, [1, 2]]]) });
    const w = source.read(ALL, [1]);
    expect(sampleAt(w, column(w, 1), 0.5, false, Number.NaN)).toBeNull();
    expect(sampleAt(w, column(w, 1), 0.5, false, -1)).toBeNull();
  });
});

describe('sampleAt on a lap distance axis', () => {
  it('interpolates by x but applies the gap rule in time', () => {
    const source = new StaticSource({
      xs: [100, 200, 300],
      ts: [10, 11, 20],
      columns: new Map([[1, [1, 3, 5]]]),
    });
    const w = source.read(ALL, [1]);
    expect(sampleAt(w, column(w, 1), 150, false, 2)).toBe(2);
    // 200 → 300 took 9 s: more than the 2 s gap limit.
    expect(sampleAt(w, column(w, 1), 250, false, 2)).toBeNull();
    // Discrete: held while the estimated time at x is within 2 s of the row (200 + 2/9 of 100 m).
    expect(sampleAt(w, column(w, 1), 220, true, 2)).toBe(3);
    expect(sampleAt(w, column(w, 1), 230, true, 2)).toBeNull();
  });

  it('uses the newest crossing when x is visited twice', () => {
    const source = new StaticSource({
      xs: [10, 20, 15, 25],
      ts: [0, 0.01, 0.02, 0.03],
      columns: new Map([[1, [1, 2, 7, 9]]]),
    });
    const w = source.read(ALL, [1]);
    expect(w.monotonicX).toBe(false);
    // 17.5 lies on 10→20 (values 1→2) and on 15→25 (values 7→9); the later visit wins.
    expect(sampleAt(w, column(w, 1), 17.5, false, 1)).toBe(7.5);
    expect(sampleAt(w, column(w, 1), 25, false, 1)).toBe(9);
    expect(sampleAt(w, column(w, 1), 30, false, 1)).toBeNull();
  });
});

describe('nearestRowX', () => {
  it('snaps to the nearest row (earlier on ties)', () => {
    const source = new StaticSource({ xs: [0, 1, 2, 4], columns: new Map() });
    const w = source.read(ALL, []);
    expect(nearestRowX(w, 1.4)).toBe(1);
    expect(nearestRowX(w, 3)).toBe(2);
    expect(nearestRowX(w, 3.1)).toBe(4);
    expect(nearestRowX(w, -10)).toBe(0);
    expect(nearestRowX(w, 99)).toBe(4);
    expect(nearestRowX(w, Number.NaN)).toBeNull();
  });

  it('works on non-monotonic axes and skips rows without x', () => {
    const source = new StaticSource({ xs: [10, Number.NaN, 30, 12], columns: new Map() });
    const w = source.read(ALL, []);
    expect(nearestRowX(w, 11.5)).toBe(12);
    expect(nearestRowX(w, 29)).toBe(30);
  });

  it('is null for an empty window', () => {
    const source = new StaticSource({ xs: [], columns: new Map() });
    expect(nearestRowX(source.read(ALL, []), 1)).toBeNull();
  });
});

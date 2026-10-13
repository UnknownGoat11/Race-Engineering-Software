import { describe, expect, it } from 'vitest';
import {
  applyAffine,
  autoRange,
  dataExtent,
  decimalsForStep,
  invertAffine,
  linearAffine,
  niceClockStep,
  niceStep,
  niceTicks,
  ticksForStep,
  unionRange,
} from './scale';

describe('linearAffine', () => {
  it('maps the domain ends onto the pixel ends, including inverted (y up) ranges', () => {
    const a = linearAffine(0, 100, 200, 0);
    expect(applyAffine(a, 0)).toBe(200);
    expect(applyAffine(a, 100)).toBe(0);
    expect(applyAffine(a, 25)).toBe(150);
    const inv = invertAffine(a);
    expect(applyAffine(inv, 150)).toBeCloseTo(25, 12);
  });
});

describe('dataExtent', () => {
  it('skips NaN and infinities and never treats a gap as zero', () => {
    expect(dataExtent([Number.NaN, 5, 7, Number.POSITIVE_INFINITY, 6], 0, 5)).toEqual({ min: 5, max: 7 });
  });

  it('returns null when there is no finite value', () => {
    expect(dataExtent([Number.NaN, Number.NaN], 0, 2)).toBeNull();
    expect(dataExtent([], 0, 0)).toBeNull();
  });

  it('only counts rows whose x is inside the visible range', () => {
    const xs = [0, 1, 2, 3];
    const ys = [100, 1, 2, -100];
    expect(dataExtent(ys, 0, 4, xs, { min: 1, max: 2 })).toEqual({ min: 1, max: 2 });
  });

  it('respects the row bounds', () => {
    expect(dataExtent([9, 1, 2, 9], 1, 3)).toEqual({ min: 1, max: 2 });
  });
});

describe('unionRange', () => {
  it('unions optional ranges', () => {
    expect(unionRange(null, null)).toBeNull();
    expect(unionRange({ min: 1, max: 2 }, null)).toEqual({ min: 1, max: 2 });
    expect(unionRange({ min: 1, max: 2 }, { min: -1, max: 1.5 })).toEqual({ min: -1, max: 2 });
  });
});

describe('nice steps and ticks', () => {
  it('chooses 1-2-5 steps', () => {
    expect(niceStep(100, 5)).toBe(20);
    expect(niceStep(297.3, 5)).toBe(100);
    expect(niceStep(1, 10)).toBeCloseTo(0.1, 12);
    expect(niceStep(0.37, 4)).toBeCloseTo(0.1, 12);
    expect(niceStep(7, 2)).toBe(5);
  });

  it('chooses clock-friendly steps from one second up', () => {
    expect(niceClockStep(60, 4)).toBe(15);
    expect(niceClockStep(180, 4)).toBe(60);
    expect(niceClockStep(400, 5)).toBe(120);
    expect(niceClockStep(2, 4)).toBeCloseTo(0.5, 12);
  });

  it('gives the decimals a step needs', () => {
    expect(decimalsForStep(100)).toBe(0);
    expect(decimalsForStep(1)).toBe(0);
    expect(decimalsForStep(0.5)).toBe(1);
    expect(decimalsForStep(0.1)).toBe(1);
    expect(decimalsForStep(0.02)).toBe(2);
    expect(decimalsForStep(0.001)).toBe(3);
  });

  it('lists the multiples of the step inside the range without binary artefacts', () => {
    expect(ticksForStep(0.05, 0.75, 0.1)).toEqual({ values: [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7], step: 0.1, decimals: 1 });
    expect(niceTicks(0, 300, 6).values).toEqual([0, 50, 100, 150, 200, 250, 300]);
    expect(niceTicks(-1, 1, 4).values).toEqual([-1, -0.5, 0, 0.5, 1]);
  });

  it('returns no ticks for degenerate input', () => {
    expect(ticksForStep(1, 0, 1).values).toEqual([]);
    expect(ticksForStep(0, 1, 0).values).toEqual([]);
    expect(ticksForStep(Number.NaN, 1, 1).values).toEqual([]);
  });
});

describe('autoRange', () => {
  it('widens a data extent to whole ticks', () => {
    expect(autoRange({ min: 3, max: 297.3 }, 5)).toEqual({ min: 0, max: 300 });
    expect(autoRange({ min: -0.42, max: 0.37 }, 4)).toEqual({ min: -0.6, max: 0.4 });
  });

  it('gives a flat series a span around its value', () => {
    const r = autoRange({ min: 50, max: 50 }, 4);
    expect(r).not.toBeNull();
    expect(r?.min).toBeLessThan(50);
    expect(r?.max).toBeGreaterThan(50);
    const z = autoRange({ min: 0, max: 0 }, 4);
    expect(z?.min).toBeLessThan(0);
    expect(z?.max).toBeGreaterThan(0);
  });

  it('returns null without data instead of inventing a range', () => {
    expect(autoRange(null, 4)).toBeNull();
  });
});

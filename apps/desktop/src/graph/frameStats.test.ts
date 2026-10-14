import { describe, expect, it, vi } from 'vitest';
import { createFrameLoop, type FrameLoopHost } from './frameLoop';
import { DurationRecorder, percentile, summarize } from './frameStats';

describe('percentile and summarize', () => {
  it('uses nearest-rank percentiles', () => {
    const sorted = Float64Array.from({ length: 100 }, (_, i) => i + 1);
    expect(percentile(sorted, 50)).toBe(50);
    expect(percentile(sorted, 95)).toBe(95);
    expect(percentile(sorted, 99)).toBe(99);
    expect(percentile(sorted, 100)).toBe(100);
    expect(percentile(sorted, 0)).toBe(1);
    expect(percentile([], 50)).toBeNaN();
  });

  it('summarizes unsorted durations and counts long frames', () => {
    const s = summarize([16, 17, 40, 15, 16, 100], 33.4);
    expect(s).toMatchObject({ count: 6, p50: 16, max: 100, long: 2 });
    expect(s.mean).toBeCloseTo(34, 9);
    expect(s.p99).toBe(100);
  });

  it('reports NaN statistics for no samples', () => {
    const s = summarize([], 33);
    expect(s.count).toBe(0);
    expect(s.mean).toBeNaN();
    expect(s.max).toBeNaN();
  });
});

describe('DurationRecorder', () => {
  it('keeps the newest values in a fixed ring, oldest first', () => {
    const r = new DurationRecorder(3);
    r.record(1);
    r.record(2);
    expect(Array.from(r.values())).toEqual([1, 2]);
    r.record(3);
    r.record(4);
    r.record(Number.NaN);
    expect(Array.from(r.values())).toEqual([2, 3, 4]);
    expect(r.count).toBe(3);
    r.reset();
    expect(r.count).toBe(0);
    expect(Array.from(r.values())).toEqual([]);
  });
});

/** A host whose frames run only when told to. */
function fakeHost(): FrameLoopHost & { runFrame: () => void; readonly requested: () => number } {
  let next = 1;
  let pending: { handle: number; callback: () => void } | null = null;
  let requested = 0;
  return {
    requestFrame: (callback) => {
      requested += 1;
      pending = { handle: next++, callback };
      return pending.handle;
    },
    cancelFrame: (handle) => {
      if (pending?.handle === handle) pending = null;
    },
    runFrame: () => {
      const p = pending;
      pending = null;
      p?.callback();
    },
    requested: () => requested,
  };
}

describe('createFrameLoop', () => {
  it('runs every callback requested before a frame in that frame, with one platform request', () => {
    const host = fakeHost();
    const schedule = createFrameLoop(host);
    const a = vi.fn();
    const b = vi.fn();
    schedule(a);
    schedule(b);
    expect(host.requested()).toBe(1);
    host.runFrame();
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);
  });

  it('runs callbacks queued during the frame in the same frame (store flush → graph repaint)', () => {
    const host = fakeHost();
    const schedule = createFrameLoop(host);
    const repaint = vi.fn();
    schedule(() => {
      schedule(repaint);
    });
    host.runFrame();
    expect(repaint).toHaveBeenCalledTimes(1);
    expect(host.requested()).toBe(1);
  });

  it('defers a callback that keeps re-queueing itself to later frames', () => {
    const host = fakeHost();
    const schedule = createFrameLoop(host);
    let runs = 0;
    const loop = (): void => {
      runs += 1;
      schedule(loop);
    };
    schedule(loop);
    host.runFrame();
    expect(runs).toBe(4);
    expect(host.requested()).toBe(2);
    host.runFrame();
    expect(runs).toBe(8);
  });

  it('keeps running the other callbacks of a frame when one throws, and reports the error', () => {
    const reported: unknown[] = [];
    const host = fakeHost();
    const schedule = createFrameLoop({ ...host, reportError: (e) => reported.push(e) });
    const after = vi.fn();
    schedule(() => {
      throw new Error('boom');
    });
    schedule(after);
    host.runFrame();
    expect(after).toHaveBeenCalledTimes(1);
    expect(reported).toEqual([new Error('boom')]);
    // The loop still schedules new frames afterwards.
    schedule(after);
    host.runFrame();
    expect(after).toHaveBeenCalledTimes(2);
  });

  it('cancels a callback and the platform frame when nothing is left', () => {
    const host = fakeHost();
    const schedule = createFrameLoop(host);
    const a = vi.fn();
    const cancel = schedule(a);
    cancel();
    host.runFrame();
    expect(a).not.toHaveBeenCalled();
    schedule(a);
    expect(host.requested()).toBe(2);
    host.runFrame();
    expect(a).toHaveBeenCalledTimes(1);
  });
});

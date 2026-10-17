import { describe, expect, it } from 'vitest';
import { findTimeGaps, findValueBreaks, tracePolyline, type PathSink, type PolylineInput } from './decimate';

type Command = readonly ['M' | 'L', number, number];

class RecordingSink implements PathSink {
  readonly commands: Command[] = [];
  moveTo(x: number, y: number): void {
    this.commands.push(['M', x, y]);
  }
  lineTo(x: number, y: number): void {
    this.commands.push(['L', x, y]);
  }
  /** Commands split into subpaths (each starts with a moveTo). */
  subpaths(): Command[][] {
    const out: Command[][] = [];
    for (const c of this.commands) {
      if (c[0] === 'M') out.push([c]);
      else out[out.length - 1]?.push(c);
    }
    return out;
  }
}

const IDENTITY = { scale: 1, offset: 0 };

function input(xs: number[], ys: number[], overrides: Partial<PolylineInput> = {}): PolylineInput {
  return {
    xs,
    ts: xs,
    ys,
    start: 0,
    end: xs.length,
    x: IDENTITY,
    y: IDENTITY,
    maxGapT: Number.POSITIVE_INFINITY,
    step: false,
    ...overrides,
  };
}

function trace(i: PolylineInput): RecordingSink {
  const sink = new RecordingSink();
  tracePolyline(i, sink);
  return sink;
}

/** Deterministic pseudo-random sequence (LCG) so failures are reproducible. */
function lcg(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 2 ** 32;
  };
}

describe('tracePolyline', () => {
  it('draws one vertex per row when every row has its own pixel column', () => {
    const sink = trace(input([0, 10, 20, 30], [5, 7, 3, 9]));
    expect(sink.commands).toEqual([
      ['M', 0.5, 5],
      ['L', 10.5, 7],
      ['L', 20.5, 3],
      ['L', 30.5, 9],
    ]);
  });

  it('breaks the line where time steps backwards (rows out of order are never joined)', () => {
    const sink = trace(input([0, 10, 20, 30], [1, 2, 3, 4], { ts: [0, 1, 0.5, 1.5], maxGapT: 10 }));
    expect(sink.subpaths()).toEqual([
      [
        ['M', 0.5, 1],
        ['L', 10.5, 2],
      ],
      [
        ['M', 20.5, 3],
        ['L', 30.5, 4],
      ],
    ]);
  });

  it('covers the minimum and maximum of rows that share a column, entering at the extreme nearer the pen', () => {
    const xs = [0, 10, 10.1, 10.2, 10.3, 10.4, 20];
    const ys = [0, 4, -8, 12, 3, 6, 0];
    const sink = trace(input(xs, ys));
    expect(sink.commands).toEqual([
      ['M', 0.5, 0],
      ['L', 10.5, -8],
      ['L', 10.5, 12],
      ['L', 20.5, 0],
    ]);
  });

  it('enters a dense column from above at its maximum, and starts a dense first column at the extreme nearer its first row', () => {
    const xs = [0, 0.2, 0.4, 10, 10.5, 20];
    const ys = [5, -3, 9, 7, 1, 2];
    expect(trace(input(xs, ys)).commands).toEqual([
      ['M', 0.5, 9],
      ['L', 0.5, -3],
      ['L', 10.5, 1],
      ['L', 10.5, 7],
      ['L', 20.5, 2],
    ]);
  });

  it('preserves every column extreme of dense random data with at most two commands per column', () => {
    const rand = lcg(42);
    const n = 20_000;
    const xs: number[] = [];
    const ys: number[] = [];
    for (let i = 0; i < n; i++) {
      xs.push(i);
      ys.push(Math.sin(i / 50) * 100 + (rand() - 0.5) * 40 + (i % 997 === 0 ? 500 : 0));
    }
    // 20 000 rows onto 400 device pixels.
    const x = { scale: 400 / n, offset: 0 };
    const sink = trace(input(xs, ys, { x }));
    const byColumn = new Map<number, { min: number; max: number }>();
    for (let i = 0; i < n; i++) {
      const col = Math.floor((xs[i] ?? 0) * x.scale) + 0.5;
      const y = ys[i] ?? 0;
      const e = byColumn.get(col);
      if (e === undefined) byColumn.set(col, { min: y, max: y });
      else byColumn.set(col, { min: Math.min(e.min, y), max: Math.max(e.max, y) });
    }
    const emitted = new Map<number, number[]>();
    for (const [, cx, cy] of sink.commands) {
      const list = emitted.get(cx) ?? [];
      list.push(cy);
      emitted.set(cx, list);
    }
    expect(byColumn.size).toBe(400);
    for (const [col, { min, max }] of byColumn) {
      const ysAt = emitted.get(col) ?? [];
      expect(ysAt).toContain(min);
      expect(ysAt).toContain(max);
      expect(Math.min(...ysAt)).toBe(min);
      expect(Math.max(...ysAt)).toBe(max);
    }
    expect(sink.commands.length).toBeLessThanOrEqual(2 * byColumn.size);
    expect(sink.subpaths()).toHaveLength(1);
  });

  it('breaks the line at NaN values and never bridges them', () => {
    const sink = trace(input([0, 10, 20, 30, 40], [1, 2, Number.NaN, 4, 5]));
    expect(sink.subpaths()).toEqual([
      [
        ['M', 0.5, 1],
        ['L', 10.5, 2],
      ],
      [
        ['M', 30.5, 4],
        ['L', 40.5, 5],
      ],
    ]);
  });

  it('breaks the line at non-finite x and time values', () => {
    const xs = [0, 10, Number.NaN, 30, 40];
    const ts = [0, 1, 2, Number.POSITIVE_INFINITY, 4];
    const sink = trace(input(xs, [1, 2, 3, 4, 5], { ts }));
    expect(sink.subpaths().map((p) => p.length)).toEqual([2, 2]);
    expect(sink.commands.at(-1)).toEqual(['L', 40.5, 5]);
  });

  it('breaks the line where consecutive row times are further apart than maxGapT', () => {
    const xs = [0, 1, 2, 6, 7];
    const sink = trace(input(xs, [1, 1, 1, 1, 1], { x: { scale: 10, offset: 0 }, maxGapT: 1 }));
    const paths = sink.subpaths();
    expect(paths).toHaveLength(2);
    expect(paths[0]?.at(-1)).toEqual(['L', 20.5, 1]);
    expect(paths[1]?.[0]).toEqual(['M', 60.5, 1]);
  });

  it('uses row times, not x, for the gap rule (lap distance axis)', () => {
    const xs = [100, 101, 102, 103];
    const ts = [10, 10.01, 15, 15.01];
    const sink = trace(input(xs, [1, 2, 3, 4], { ts, x: { scale: 10, offset: 0 }, maxGapT: 0.5 }));
    expect(sink.subpaths()).toHaveLength(2);
  });

  it('bridges a step exactly equal to maxGapT', () => {
    const sink = trace(input([0, 1, 2], [1, 2, 3], { x: { scale: 10, offset: 0 }, maxGapT: 1 }));
    expect(sink.subpaths()).toHaveLength(1);
  });

  it('draws discrete channels as steps: hold, then jump', () => {
    const sink = trace(input([0, 10, 20, 30], [0, 1, 1, 0], { step: true }));
    expect(sink.commands).toEqual([
      ['M', 0.5, 0],
      ['L', 10.5, 0],
      ['L', 10.5, 1],
      ['L', 20.5, 1],
      ['L', 30.5, 1],
      ['L', 30.5, 0],
    ]);
  });

  it('does not extend a step past the last sample before a gap', () => {
    const sink = trace(input([0, 10, 20, 30], [2, 3, Number.NaN, 4], { step: true }));
    expect(sink.subpaths()).toEqual([
      [
        ['M', 0.5, 2],
        ['L', 10.5, 2],
        ['L', 10.5, 3],
      ],
      [
        ['M', 30.5, 4],
        ['L', 30.5, 4],
      ],
    ]);
  });

  it('draws an isolated sample as a zero-length segment (a dot)', () => {
    const sink = trace(input([0, 10, 20], [Number.NaN, 7, Number.NaN]));
    expect(sink.commands).toEqual([
      ['M', 10.5, 7],
      ['L', 10.5, 7],
    ]);
  });

  it('traces only rows [start, end)', () => {
    const sink = trace(input([0, 10, 20, 30], [1, 2, 3, 4], { start: 1, end: 3 }));
    expect(sink.commands).toEqual([
      ['M', 10.5, 2],
      ['L', 20.5, 3],
    ]);
  });

  it('handles x that goes backwards without merging distant rows', () => {
    const sink = trace(input([10, 20, 15, 30], [1, 2, 3, 4]));
    expect(sink.commands.map((c) => c[1])).toEqual([10.5, 20.5, 15.5, 30.5]);
  });

  it('clamps far off-plot coordinates', () => {
    const sink = trace(input([0, 1], [0, 1e300], { x: { scale: 1, offset: 0 } }));
    expect(sink.commands[1]).toEqual(['L', 1.5, 1e6]);
  });
});

describe('findTimeGaps', () => {
  it('reports x intervals between rows further apart in time than maxGapT, merged', () => {
    const ts = [0, 0.01, 2, 4, 4.01, 9];
    const xs = ts;
    expect(findTimeGaps(xs, ts, 0, ts.length, 0.5)).toEqual([
      { min: 0.01, max: 4 },
      { min: 4.01, max: 9 },
    ]);
  });

  it('ignores rows without a finite time or x and gaps within maxGapT', () => {
    const ts = [0, 0.1, Number.NaN, 0.3];
    expect(findTimeGaps(ts, ts, 0, ts.length, 0.5)).toEqual([]);
  });

  it('measures gaps in time but reports them in x (lap distance)', () => {
    const ts = [10, 10.01, 15];
    const xs = [100, 101, 102];
    expect(findTimeGaps(xs, ts, 0, 3, 1)).toEqual([{ min: 101, max: 102 }]);
  });
});

describe('gap and break detection (silent-failure M2)', () => {
  it('skips rows without data, so a short drop split by the store\'s midpoint gap row still shows a gap', () => {
    // Rows 1.00 and 1.31 around a 0.31 s drop; the store put an all-NaN row at 1.155 (each half < maxGapT 0.25).
    const ts = [0.99, 1.0, 1.155, 1.31, 1.32];
    expect(findTimeGaps(ts, ts, 0, ts.length, 0.25)).toEqual([]);
    expect(findTimeGaps(ts, ts, 0, ts.length, 0.25, [], (i) => i === 2)).toEqual([{ min: 1.0, max: 1.31 }]);
  });

  it('reports a gap on an axis whose x steps back (lap axis)', () => {
    const ts = [10, 10.01, 15];
    const xs = [500, 501, 300];
    expect(findTimeGaps(xs, ts, 0, 3, 1)).toEqual([{ min: 300, max: 501 }]);
  });

  it('finds interior value runs of one series, not leading or trailing ones', () => {
    const xs = [0, 1, 2, 3, 4, 5, 6, 7];
    const ys = [Number.NaN, 1, Number.NaN, Number.NaN, 2, 3, Number.NaN, Number.NaN];
    expect(findValueBreaks(xs, ys, 0, xs.length)).toEqual([{ min: 1, max: 4 }]);
    expect(findValueBreaks(xs, [1, 2, 3, 4, 5, 6, 7, 8], 0, xs.length)).toEqual([]);
  });
});

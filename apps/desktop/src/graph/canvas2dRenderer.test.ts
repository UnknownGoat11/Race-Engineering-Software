import { describe, expect, it } from 'vitest';
import { RecordingContext, type RecordedCall } from '../test/recordingContext';
import { Canvas2DRenderer, drawSeriesFrame } from './canvas2dRenderer';
import type { RenderFrame, RenderSeries } from './renderer';
import type { GraphWindow } from './types';

function windowOf(xs: readonly number[], ts?: readonly number[]): GraphWindow {
  const x = Float64Array.from(xs);
  const t = ts === undefined ? x : Float64Array.from(ts);
  let monotonic = true;
  for (let i = 1; i < xs.length; i++) if (!((xs[i] ?? Number.NaN) >= (xs[i - 1] ?? Number.NaN))) monotonic = false;
  return { xs: x, ts: t, count: xs.length, monotonicX: monotonic, column: () => null };
}

function series(window: GraphWindow, ys: readonly number[], extra: Partial<RenderSeries> = {}): RenderSeries {
  return {
    key: 's',
    window,
    ys: Float64Array.from(ys),
    yRange: { min: 0, max: 2 },
    color: '#123456',
    dashed: false,
    step: false,
    ...extra,
  };
}

/** Plot 30 × 10 CSS px at the origin, x 0…3 → 0…30 px, y 0…2 → 10…0 px. */
function frame(s: readonly RenderSeries[], extra: Partial<RenderFrame> = {}): RenderFrame {
  return {
    plot: { left: 0, top: 0, width: 30, height: 10 },
    xRange: { min: 0, max: 3 },
    maxGapT: Number.POSITIVE_INFINITY,
    series: s,
    ...extra,
  };
}

function path(calls: readonly RecordedCall[]): string[] {
  return calls.filter((c) => c.op === 'moveTo' || c.op === 'lineTo').map((c) => `${c.op} ${c.args.join(',')}`);
}

describe('drawSeriesFrame', () => {
  it('draws a Discrete channel as steps: hold the value, then jump vertically (never a diagonal)', () => {
    const ctx = new RecordingContext();
    drawSeriesFrame(ctx, frame([series(windowOf([0, 1, 2, 3]), [1, 1, 2, 2], { step: true })]), 1);
    expect(path(ctx.calls)).toEqual([
      'moveTo 0.5,5',
      'lineTo 10.5,5',
      'lineTo 20.5,5',
      'lineTo 20.5,0',
      'lineTo 30.5,0',
    ]);
    const points = ctx.calls.filter((c) => c.op === 'moveTo' || c.op === 'lineTo').map((c) => c.args as number[]);
    for (let i = 1; i < points.length; i++) {
      const [x0, y0] = points[i - 1] ?? [];
      const [x1, y1] = points[i] ?? [];
      expect(x0 === x1 || y0 === y1).toBe(true);
    }
  });

  it('draws a Continuous channel with straight segments between rows', () => {
    const ctx = new RecordingContext();
    drawSeriesFrame(ctx, frame([series(windowOf([0, 1, 2, 3]), [1, 1, 2, 2])]), 1);
    expect(path(ctx.calls)).toEqual(['moveTo 0.5,5', 'lineTo 10.5,5', 'lineTo 20.5,0', 'lineTo 30.5,0']);
  });

  it('breaks the line at a NaN value and at a time gap (no line across missing data)', () => {
    const ctx = new RecordingContext();
    const w = windowOf([0, 1, 2, 3], [0, 1, 2, 10]);
    drawSeriesFrame(ctx, frame([series(w, [1, Number.NaN, 1, 1])], { maxGapT: 1 }), 1);
    const moves = ctx.ops('moveTo').map((c) => c.args[0]);
    // A new run starts after the NaN row and after the 8 s time gap.
    expect(moves).toEqual([0.5, 20.5, 30.5]);
    const lines = ctx.ops('lineTo');
    for (const l of lines) {
      const x = l.args[0] as number;
      // Every lineTo stays on the column of its own run (single-row runs are drawn as dots).
      expect([0.5, 20.5, 30.5]).toContain(x);
    }
  });

  it('clips to the plot, strokes each series in its colour and dashes reference overlays', () => {
    const ctx = new RecordingContext();
    ctx.lineWidth = 7;
    const w = windowOf([0, 1, 2, 3]);
    drawSeriesFrame(
      ctx,
      frame([series(w, [0, 1, 2, 1], { color: '#aaa' }), series(w, [1, 1, 1, 1], { key: 'ref', color: '#bbb', dashed: true })], {
        plot: { left: 5, top: 4, width: 30, height: 10 },
      }),
      2,
    );
    const rect = ctx.ops('rect')[0];
    expect(rect?.args).toEqual([10, 8, 60, 20]);
    const firstClip = ctx.calls.findIndex((c) => c.op === 'clip');
    const firstStroke = ctx.calls.findIndex((c) => c.op === 'stroke');
    expect(firstClip).toBeGreaterThan(-1);
    expect(firstClip).toBeLessThan(firstStroke);
    const strokes = ctx.ops('stroke');
    expect(strokes.map((s) => s.style)).toEqual(['#aaa', '#bbb']);
    expect(strokes[0]?.dash).toEqual([]);
    expect(strokes[1]?.dash).toEqual([12, 8]);
    // Hairline (1 device px) at any pixel ratio: Skia's fast path (see SERIES_LINE_WIDTH_DEVICE_PX).
    expect(ctx.lineWidth).toBe(1);
    expect(ctx.calls.at(-1)?.op).toBe('restore');
  });

  it('draws only the rows of the visible range plus one beyond each edge', () => {
    const ctx = new RecordingContext();
    const xs = Array.from({ length: 101 }, (_, i) => i);
    drawSeriesFrame(ctx, frame([series(windowOf(xs), xs.map(() => 1))], { xRange: { min: 20, max: 30 } }), 1);
    const xsDrawn = ctx.calls.filter((c) => c.op === 'moveTo' || c.op === 'lineTo').map((c) => c.args[0] as number);
    // x 19 → -3 px (column -3) and x 31 → 33 px are the edge rows; nothing further out is traced.
    expect(Math.min(...xsDrawn)).toBe(-2.5);
    expect(Math.max(...xsDrawn)).toBe(33.5);
  });

  it('draws nothing for an empty plot, an empty x range or a degenerate y range', () => {
    const ctx = new RecordingContext();
    const s = series(windowOf([0, 1]), [1, 2]);
    expect(drawSeriesFrame(ctx, frame([s], { plot: { left: 0, top: 0, width: 0, height: 10 } }), 1)).toBe(0);
    expect(drawSeriesFrame(ctx, frame([s], { xRange: { min: 1, max: 1 } }), 1)).toBe(0);
    expect(drawSeriesFrame(ctx, frame([{ ...s, yRange: { min: 1, max: 1 } }]), 1)).toBe(0);
    expect(ctx.ops('lineTo')).toEqual([]);
  });
});

describe('Canvas2DRenderer', () => {
  it('mounts a canvas sized for the device pixel ratio, clears it each frame and unmounts on destroy', () => {
    const ctx = new RecordingContext();
    const host = document.createElement('div');
    const renderer = new Canvas2DRenderer({ context: () => ctx });
    renderer.mount(host);
    renderer.setSize(200, 100, 1.5);
    const canvas = host.querySelector('canvas');
    expect(canvas?.width).toBe(300);
    expect(canvas?.height).toBe(150);
    expect(canvas?.style.width).toBe('200px');
    renderer.render(frame([series(windowOf([0, 1]), [1, 2])]));
    expect(ctx.ops('clearRect')[0]?.args).toEqual([0, 0, 300, 150]);
    expect(ctx.ops('stroke')).toHaveLength(1);
    renderer.destroy();
    expect(host.querySelector('canvas')).toBeNull();
  });
});

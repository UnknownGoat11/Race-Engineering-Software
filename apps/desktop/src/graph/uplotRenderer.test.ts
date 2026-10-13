import { afterEach, describe, expect, it, vi } from 'vitest';
import type uPlot from 'uplot';
import type { RenderFrame, RenderSeries } from './renderer';
import type { GraphWindow } from './types';
import { UplotRenderer, alignedBuffers, fillAlignedData } from './uplotRenderer';

function windowOf(xs: readonly number[], ts?: readonly number[]): GraphWindow {
  const x = Float64Array.from(xs);
  return { xs: x, ts: ts === undefined ? x : Float64Array.from(ts), count: xs.length, monotonicX: true, column: () => null };
}

describe('fillAlignedData', () => {
  it('converts NaN values to null (uPlot breaks lines only at null)', () => {
    const out = alignedBuffers(1);
    const n = fillAlignedData(windowOf([0, 1, 2]), [Float64Array.from([1, Number.NaN, 3])], 0, 3, 10, out);
    expect(n).toBe(3);
    expect(out.xs).toEqual([0, 1, 2]);
    expect(out.ys[0]).toEqual([1, null, 3]);
  });

  it('inserts an all-null row at the midpoint of a time gap larger than maxGapT', () => {
    const out = alignedBuffers(2);
    const w = windowOf([0, 1, 2, 3], [0, 0.01, 4.01, 4.02]);
    const cols = [Float64Array.from([1, 2, 3, 4]), Float64Array.from([5, 6, 7, 8])];
    expect(fillAlignedData(w, cols, 0, 4, 0.25, out)).toBe(5);
    expect(out.xs).toEqual([0, 1, 1.5, 2, 3]);
    expect(out.ys[0]).toEqual([1, 2, null, 3, 4]);
    expect(out.ys[1]).toEqual([5, 6, null, 7, 8]);
  });

  it('turns a row without x into a null row at the previous x, keeping x ascending', () => {
    const out = alignedBuffers(1);
    const w = windowOf([0, Number.NaN, 2]);
    fillAlignedData(w, [Float64Array.from([1, 2, 3])], 0, 3, 10, out);
    expect(out.xs).toEqual([0, 0, 2]);
    expect(out.ys[0]).toEqual([1, null, 3]);
    expect(out.sorted).toBe(true);
  });

  it('reports whether finite x stayed sorted and breaks lines where time steps back', () => {
    const out = alignedBuffers(1);
    fillAlignedData(windowOf([0, 1, 2]), [Float64Array.from([1, 2, 3])], 0, 3, 10, out);
    expect(out.sorted).toBe(true);
    fillAlignedData(windowOf([0, 2, 1.5], [0, 0.02, 0.01]), [Float64Array.from([1, 2, 3])], 0, 3, 10, out);
    expect(out.sorted).toBe(false);
    // The backwards time step got an all-null row before the third row.
    expect(out.ys[0]).toEqual([1, 2, null, 3]);
  });

  it('reuses and shrinks the buffers and honours [start, end)', () => {
    const out = alignedBuffers(1);
    const xsBefore = out.xs;
    fillAlignedData(windowOf([0, 1, 2, 3]), [Float64Array.from([1, 2, 3, 4])], 0, 4, 10, out);
    fillAlignedData(windowOf([0, 1, 2, 3]), [Float64Array.from([1, 2, 3, 4])], 1, 3, 10, out);
    expect(out.xs).toBe(xsBefore);
    expect(out.xs).toEqual([1, 2]);
    expect(out.ys[0]).toEqual([2, 3]);
  });
});

interface FakeInstance {
  readonly opts: uPlot.Options;
  readonly calls: string[];
  data: uPlot.AlignedData;
  destroyed: boolean;
}

function fakeUPlot(): { readonly ctor: typeof uPlot; readonly instances: FakeInstance[] } {
  const instances: FakeInstance[] = [];
  class Fake {
    readonly root = document.createElement('div');
    width: number;
    height: number;
    readonly #rec: FakeInstance;
    readonly bbox = { left: 0, top: 0, width: 300, height: 100 };
    static readonly paths = {
      linear: () => () => 'linear-paths',
      stepped: () => () => 'stepped-paths',
    };
    constructor(opts: uPlot.Options, data: uPlot.AlignedData, target: HTMLElement) {
      this.width = opts.width;
      this.height = opts.height;
      this.#rec = { opts, calls: [], data, destroyed: false };
      instances.push(this.#rec);
      target.appendChild(this.root);
    }
    batch(fn: () => void): void {
      this.#rec.calls.push('batch');
      fn();
    }
    setSize(size: { width: number; height: number }): void {
      this.width = size.width;
      this.height = size.height;
      this.#rec.calls.push(`setSize ${size.width}x${size.height}`);
    }
    setData(data: uPlot.AlignedData, reset: boolean): void {
      this.#rec.data = data;
      this.#rec.calls.push(`setData ${String(reset)}`);
    }
    setScale(key: string, limits: { min: number; max: number }): void {
      this.#rec.calls.push(`setScale ${key} ${limits.min} ${limits.max}`);
    }
    destroy(): void {
      this.#rec.destroyed = true;
      this.root.remove();
    }
  }
  return { ctor: Fake as unknown as typeof uPlot, instances };
}

function series(window: GraphWindow, key: string, extra: Partial<RenderSeries> = {}): RenderSeries {
  return {
    key,
    window,
    ys: Float64Array.from([1, 2, 3]),
    yRange: { min: 0, max: 4 },
    color: '#abc',
    dashed: false,
    step: false,
    ...extra,
  };
}

function frame(s: readonly RenderSeries[]): RenderFrame {
  return { plot: { left: 40, top: 20, width: 300, height: 100 }, xRange: { min: 0, max: 2 }, maxGapT: 1, series: s };
}

describe('UplotRenderer', () => {
  it('draws pending frames once uPlot has loaded, one instance per window, with exact scales', async () => {
    const fake = fakeUPlot();
    const renderer = new UplotRenderer(() => Promise.resolve(fake.ctor));
    const host = document.createElement('div');
    renderer.mount(host);
    renderer.setSize(400, 150);
    const main = windowOf([0, 1, 2]);
    const reference = windowOf([0, 1, 2]);
    renderer.render(
      frame([
        series(main, 'a', { step: true }),
        series(main, 'b', { yRange: { min: -1, max: 1 } }),
        series(reference, 'r', { dashed: true }),
      ]),
    );
    expect(fake.instances).toHaveLength(0);
    await renderer.ready;
    expect(fake.instances).toHaveLength(2);
    const [mainPlot, refPlot] = fake.instances;
    expect(mainPlot?.opts.series).toHaveLength(3);
    // Sorted x: uPlot's own builders (stepped for Discrete channels, linear otherwise).
    const u = {} as uPlot;
    expect(mainPlot?.opts.series[1]?.paths?.(u, 1, 0, 2)).toBe('stepped-paths');
    expect(mainPlot?.opts.series[2]?.paths?.(u, 2, 0, 2)).toBe('linear-paths');
    expect(refPlot?.opts.series[1]?.dash).toEqual([6, 4]);
    expect(mainPlot?.opts.legend?.show).toBe(false);
    expect(mainPlot?.opts.cursor?.show).toBe(false);
    expect(mainPlot?.calls).toEqual([
      'batch',
      'setData false',
      'setScale x 0 2',
      'setScale y0 0 4',
      'setScale y1 -1 1',
    ]);
    const range = mainPlot?.opts.scales?.['y0']?.range;
    expect(typeof range === 'function' ? range({} as uPlot, 3, 7, 'y0') : null).toEqual([3, 7]);
    expect(host.querySelectorAll('.telemetry-graph__uplot-group')).toHaveLength(2);
  });

  it('strokes exactly one device pixel (hairline) and rebuilds the instances when the pixel ratio changes', async () => {
    const fake = fakeUPlot();
    const renderer = new UplotRenderer(() => Promise.resolve(fake.ctor));
    await renderer.ready;
    renderer.mount(document.createElement('div'));
    const w = windowOf([0, 1, 2]);
    renderer.setSize(400, 150, 2);
    renderer.render(frame([series(w, 'a')]));
    expect(fake.instances[0]?.opts.series[1]?.width).toBe(0.5);
    renderer.render(frame([series(w, 'a')]));
    expect(fake.instances).toHaveLength(1);
    renderer.setSize(400, 150, 1.25);
    renderer.render(frame([series(w, 'a')]));
    expect(fake.instances).toHaveLength(2);
    expect(fake.instances[0]?.destroyed).toBe(true);
    expect(fake.instances[1]?.opts.series[1]?.width).toBe(0.8);
  });

  it('reuses instances while the series are unchanged and replaces them when they change', async () => {
    const fake = fakeUPlot();
    const renderer = new UplotRenderer(() => Promise.resolve(fake.ctor));
    await renderer.ready;
    const host = document.createElement('div');
    renderer.mount(host);
    const w = windowOf([0, 1, 2]);
    renderer.render(frame([series(w, 'a')]));
    renderer.render(frame([series(w, 'a')]));
    expect(fake.instances).toHaveLength(1);
    renderer.render(frame([series(w, 'a'), series(w, 'b')]));
    expect(fake.instances).toHaveLength(2);
    expect(fake.instances[0]?.destroyed).toBe(true);
    renderer.destroy();
    expect(fake.instances[1]?.destroyed).toBe(true);
    expect(host.children).toHaveLength(0);
  });

  it('traces a window whose x steps back itself instead of letting uPlot window it (car reversing)', async () => {
    class RecordingPath {
      readonly commands: (readonly [string, number, number])[] = [];
      moveTo(x: number, y: number): void {
        this.commands.push(['M', x, y]);
      }
      lineTo(x: number, y: number): void {
        this.commands.push(['L', x, y]);
      }
    }
    vi.stubGlobal('Path2D', RecordingPath);
    const fake = fakeUPlot();
    const renderer = new UplotRenderer(() => Promise.resolve(fake.ctor));
    await renderer.ready;
    renderer.mount(document.createElement('div'));
    const xs = Float64Array.from([0, 1, 2, 1.5, 3]);
    const window: GraphWindow = {
      xs,
      ts: Float64Array.from([0, 0.01, 0.02, 0.03, 0.04]),
      count: 5,
      monotonicX: false,
      column: () => null,
    };
    renderer.render({
      plot: { left: 0, top: 0, width: 300, height: 100 },
      xRange: { min: 0, max: 3 },
      maxGapT: 1,
      series: [series(window, 'a', { ys: Float64Array.from([1, 2, 3, 4, 5]) })],
    });
    const u = { bbox: { left: 0, top: 0, width: 300, height: 100 } } as unknown as uPlot;
    const paths = fake.instances[0]?.opts.series[1]?.paths?.(u, 1, 0, 4);
    const stroke = paths !== null && typeof paths === 'object' ? paths.stroke : null;
    expect(stroke).toBeInstanceOf(RecordingPath);
    const commands = stroke instanceof RecordingPath ? stroke.commands : [];
    expect(commands.map((c) => c[1])).toEqual([0.5, 100.5, 200.5, 150.5, 300.5]);
    expect(commands.map((c) => c[2])).toEqual([75, 50, 25, 0, -25]);
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});
